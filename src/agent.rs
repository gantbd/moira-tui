//! Reusable read-only presentation of one provider-neutral agent session.

use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

use moira_client::{AgentReadOnlyClient, AgentServicePort};
use moira_protocol::agent;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::{Input, Lifecycle};

const EVENT_PAGE_LIMIT: u64 = 64;
const PREVIEW_BYTES: usize = 48;

enum Observation {
    Current {
        session: Box<agent::SessionViewV1>,
        page: Box<agent::EventPageV1>,
        after: Option<u64>,
    },
    Stale,
    OutcomeUnknown,
    Refused(String),
    InvalidReply,
    Transport,
}

pub struct AgentComponent {
    requests: SyncSender<Option<u64>>,
    replies: Receiver<Observation>,
    pending: bool,
    session: Option<agent::SessionViewV1>,
    page: Option<agent::EventPageV1>,
    after: Option<u64>,
    selected: usize,
    notice: String,
}

impl AgentComponent {
    pub fn new<P>(port: P, session: agent::SessionIdV1) -> Self
    where
        P: AgentServicePort + Send + 'static,
        P::Error: Send + 'static,
    {
        let (request_tx, request_rx) = sync_channel::<Option<u64>>(1);
        let (reply_tx, reply_rx) = sync_channel::<Observation>(1);
        std::thread::spawn(move || {
            let mut client = AgentReadOnlyClient::new(port);
            while let Ok(after) = request_rx.recv() {
                if reply_tx.send(observe(&mut client, session, after)).is_err() {
                    break;
                }
            }
        });
        let mut component = Self {
            requests: request_tx,
            replies: reply_rx,
            pending: false,
            session: None,
            page: None,
            after: None,
            selected: 0,
            notice: "Provider-neutral agent session ready; refresh queued".into(),
        };
        component.offer(None);
        component
    }

    pub fn poll(&mut self) {
        loop {
            match self.replies.try_recv() {
                Ok(observation) => self.admit(observation),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pending = false;
                    self.notice = "Moira agent worker unavailable".into();
                    break;
                }
            }
        }
    }

    pub fn update(&mut self, input: Input, focused: bool) -> Lifecycle {
        if !focused {
            return Lifecycle::Running;
        }
        match input {
            Input::Close => Lifecycle::CloseRequested,
            Input::Refresh => {
                self.offer(self.after);
                Lifecycle::Running
            }
            Input::Up => {
                self.selected = self.selected.saturating_sub(1);
                Lifecycle::Running
            }
            Input::Down => {
                let length = self.page.as_ref().map_or(0, |page| page.entries.len());
                if self.selected + 1 < length {
                    self.selected += 1;
                }
                Lifecycle::Running
            }
            Input::Open => {
                if let Some(after) = self.page.as_ref().and_then(|page| page.next_after) {
                    self.offer(Some(after));
                }
                Lifecycle::Running
            }
            Input::Back => {
                if let Some(after) = self.after {
                    self.offer(after.checked_sub(EVENT_PAGE_LIMIT));
                }
                Lifecycle::Running
            }
            Input::Toggle => Lifecycle::Running,
        }
    }

    pub fn draw_in(&self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let [body, controls, notice] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let title = if focused {
            " Moira agent session • focused "
        } else {
            " Moira agent session "
        };
        let lines = match (&self.session, &self.page) {
            (Some(session), Some(page)) => agent_lines(
                session,
                page,
                self.selected,
                usize::from(body.height.saturating_sub(2)),
            ),
            _ => vec![Line::from("No stable provider-neutral agent session")],
        };
        frame.render_widget(
            Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
            body,
        );
        frame.render_widget(
            Paragraph::new(
                "j/k selects • Enter next page • Backspace previous • r refreshes • q closes",
            ),
            controls,
        );
        frame.render_widget(Paragraph::new(self.notice.as_str()), notice);
    }

    fn offer(&mut self, after: Option<u64>) {
        if self.pending {
            self.notice = "Moira agent request already pending".into();
        } else if self.requests.try_send(after).is_ok() {
            self.pending = true;
            self.notice = "Moira agent refresh pending".into();
        } else {
            self.notice = "Moira agent worker unavailable".into();
        }
    }

    fn admit(&mut self, observation: Observation) {
        self.pending = false;
        match observation {
            Observation::Current {
                session,
                page,
                after,
            } => {
                self.selected = self.selected.min(page.entries.len().saturating_sub(1));
                self.notice = format!("Stable agent revision {}", session.revision);
                self.session = Some(*session);
                self.page = Some(*page);
                self.after = after;
            }
            Observation::Stale => {
                self.notice = "Agent changed during observation; refresh required".into();
            }
            Observation::OutcomeUnknown => {
                self.notice = "Moira reports agent outcome unknown; state not inferred".into();
            }
            Observation::Refused(refusal) => {
                self.notice = format!("Moira refused: {refusal}");
            }
            Observation::InvalidReply => {
                self.notice = "Invalid Moira agent reply; state not inferred".into();
            }
            Observation::Transport => {
                self.notice = "Moira unavailable; agent state not inferred".into();
            }
        }
    }
}

fn observe<P: AgentServicePort>(
    client: &mut AgentReadOnlyClient<P>,
    session: agent::SessionIdV1,
    after: Option<u64>,
) -> Observation {
    let first = match read_session(client.inspect(session)) {
        Ok(value) => value,
        Err(observation) => return observation,
    };
    let page = match client.events(session, after, EVENT_PAGE_LIMIT) {
        Ok(agent::ReplyV1::Accepted(agent::OutcomeV1::Events(page))) => page,
        Ok(agent::ReplyV1::Accepted(agent::OutcomeV1::OutcomeUnknown { .. })) => {
            return Observation::OutcomeUnknown;
        }
        Ok(agent::ReplyV1::Accepted(_)) => return Observation::InvalidReply,
        Ok(agent::ReplyV1::Refused(refusal)) => {
            return Observation::Refused(format!("{refusal:?}"));
        }
        Err(_) => return Observation::Transport,
    };
    let second = match read_session(client.inspect(session)) {
        Ok(value) => value,
        Err(observation) => return observation,
    };
    if first != second {
        return Observation::Stale;
    }
    if !valid_page(&first, &page, after) {
        return Observation::InvalidReply;
    }
    Observation::Current {
        session: Box::new(first),
        page: Box::new(page),
        after,
    }
}

fn read_session<E>(reply: Result<agent::ReplyV1, E>) -> Result<agent::SessionViewV1, Observation> {
    match reply {
        Ok(agent::ReplyV1::Accepted(agent::OutcomeV1::Session(view))) => Ok(view),
        Ok(agent::ReplyV1::Accepted(agent::OutcomeV1::OutcomeUnknown { .. })) => {
            Err(Observation::OutcomeUnknown)
        }
        Ok(agent::ReplyV1::Accepted(_)) => Err(Observation::InvalidReply),
        Ok(agent::ReplyV1::Refused(refusal)) => Err(Observation::Refused(format!("{refusal:?}"))),
        Err(_) => Err(Observation::Transport),
    }
}

fn valid_page(
    session: &agent::SessionViewV1,
    page: &agent::EventPageV1,
    after: Option<u64>,
) -> bool {
    if page.session != session.session || page.entries.len() > EVENT_PAGE_LIMIT as usize {
        return false;
    }
    let mut expected = after.map_or(Some(0), |cursor| cursor.checked_add(1));
    for record in &page.entries {
        if Some(record.cursor) != expected {
            return false;
        }
        expected = record.cursor.checked_add(1);
    }
    let last = page.entries.last().map(|record| record.cursor);
    match page.next_after {
        Some(cursor) => {
            last == Some(cursor)
                && session
                    .last_event_cursor
                    .is_some_and(|session_cursor| cursor < session_cursor)
        }
        None => last == session.last_event_cursor,
    }
}

fn agent_lines(
    session: &agent::SessionViewV1,
    page: &agent::EventPageV1,
    selected: usize,
    capacity: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(format!(
            "phase: {:?} • revision: {} • active turn: {}",
            session.phase,
            session.revision,
            session.active_turn.is_some()
        )),
        Line::from(format!(
            "last cursor: {:?} • after: {:?} • events shown: {} • {}",
            session.last_event_cursor,
            page.entries
                .first()
                .and_then(|record| record.cursor.checked_sub(1)),
            page.entries.len(),
            if page.next_after.is_some() {
                "TRUNCATED"
            } else {
                "complete"
            }
        )),
        Line::from(format!(
            "capabilities: permission={} read={} write={} terminal={}",
            session.capabilities.permissions,
            session.capabilities.filesystem_read,
            session.capabilities.filesystem_write,
            session.capabilities.terminal
        )),
        Line::from(format!(
            "session capabilities: load={} resume={} modes={} models={}",
            session.capabilities.load_session,
            session.capabilities.resume_session,
            session.capabilities.session_modes,
            session.capabilities.session_models
        )),
    ];
    if session.phase == agent::PhaseV1::ReattachRequired {
        lines.push(Line::from(
            "recovery: explicit governed reattach required; automatic resume disabled",
        ));
    }
    let slots = capacity.saturating_sub(lines.len());
    if slots == 0 || page.entries.is_empty() {
        if page.entries.is_empty() {
            lines.push(Line::from("no retained events"));
        }
        return lines;
    }
    let start = selected.saturating_add(1).saturating_sub(slots);
    for (index, record) in page.entries.iter().enumerate().skip(start).take(slots) {
        let marker = if index == selected { ">" } else { " " };
        lines.push(Line::from(format!(
            "{marker} {:>5} {}",
            record.cursor,
            event_summary(&record.event)
        )));
    }
    lines
}

fn event_summary(event: &agent::EventV1) -> String {
    match event {
        agent::EventV1::AssistantMessage { bytes } => {
            format!("assistant: {}", escaped_preview(bytes))
        }
        agent::EventV1::Thought { bytes } => format!("thought: {}", escaped_preview(bytes)),
        agent::EventV1::Plan { bytes } => format!("plan: {}", escaped_preview(bytes)),
        agent::EventV1::ToolCall { title, detail } => format!(
            "tool: {} • {}",
            escaped_preview(title),
            escaped_preview(detail)
        ),
        agent::EventV1::Terminal { stream, bytes } => format!(
            "terminal[{}]: {}",
            escaped_preview(stream),
            escaped_preview(bytes)
        ),
        agent::EventV1::Permission(value) => format!(
            "permission[{} option(s)]: {} • {}",
            value.options.len(),
            escaped_preview(&value.capability),
            escaped_preview(&value.description)
        ),
        agent::EventV1::FileChange { path, .. } => {
            format!("file change: {} • digest retained", escaped_preview(path))
        }
        agent::EventV1::FileRead {
            path,
            content_bytes,
            ..
        } => format!(
            "file read: {} • {content_bytes} byte(s) • digest retained",
            escaped_preview(path)
        ),
        agent::EventV1::Usage {
            input_tokens,
            output_tokens,
        } => format!("usage: input={input_tokens} output={output_tokens}"),
        agent::EventV1::TurnFinished {
            disposition,
            stop_reason,
            output_digest,
            ..
        } => format!(
            "turn finished: {disposition:?}/{stop_reason:?} • output digest={}",
            output_digest.is_some()
        ),
    }
}

fn escaped_preview(bytes: &[u8]) -> String {
    let mut value = String::new();
    for byte in bytes.iter().copied().take(PREVIEW_BYTES) {
        match byte {
            b'\\' => value.push_str("\\\\"),
            0x20..=0x7e => value.push(char::from(byte)),
            _ => value.push_str(&format!("\\x{byte:02x}")),
        }
    }
    if bytes.len() > PREVIEW_BYTES {
        value.push_str(&format!("…(+{} bytes)", bytes.len() - PREVIEW_BYTES));
    }
    value
}

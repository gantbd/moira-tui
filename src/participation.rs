//! Reusable read-only presentation of one governed agent participation.

use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

use moira_client::{AgentParticipationReadOnlyClient, AgentParticipationServicePort};
use moira_protocol::agent_participation as participation;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::{Input, Lifecycle};

enum Observation {
    Current(Box<participation::ParticipationViewV1>),
    NotFound,
    Refused(String),
    InvalidReply,
    Transport,
}

pub struct ParticipationComponent {
    requests: SyncSender<()>,
    replies: Receiver<Observation>,
    pending: bool,
    view: Option<participation::ParticipationViewV1>,
    notice: String,
}

impl ParticipationComponent {
    pub fn new<P>(port: P, participation: participation::ParticipationIdV1) -> Self
    where
        P: AgentParticipationServicePort + Send + 'static,
        P::Error: Send + 'static,
    {
        let (request_tx, request_rx) = sync_channel::<()>(1);
        let (reply_tx, reply_rx) = sync_channel::<Observation>(1);
        std::thread::spawn(move || {
            let mut client = AgentParticipationReadOnlyClient::new(port);
            while request_rx.recv().is_ok() {
                let observation = observe(&mut client, participation);
                if reply_tx.send(observation).is_err() {
                    break;
                }
            }
        });
        let mut component = Self {
            requests: request_tx,
            replies: reply_rx,
            pending: false,
            view: None,
            notice: "Governed participation ready; refresh queued".into(),
        };
        component.offer();
        component
    }

    pub fn poll(&mut self) {
        loop {
            match self.replies.try_recv() {
                Ok(observation) => self.admit(observation),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pending = false;
                    self.notice = "Moira participation worker unavailable".into();
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
                self.offer();
                Lifecycle::Running
            }
            Input::Up | Input::Down | Input::Open | Input::Back => Lifecycle::Running,
        }
    }

    pub fn draw_in(&self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        let title = if focused {
            " Moira agent participation • focused "
        } else {
            " Moira agent participation "
        };
        let lines = self.view.as_ref().map_or_else(
            || vec![Line::from("No current governed participation")],
            participation_lines,
        );
        frame.render_widget(
            Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
            body,
        );
        frame.render_widget(Paragraph::new(self.notice.as_str()), footer);
    }

    fn offer(&mut self) {
        if self.pending {
            self.notice = "Moira participation request already pending".into();
        } else if self.requests.try_send(()).is_ok() {
            self.pending = true;
            self.notice = "Moira participation refresh pending".into();
        } else {
            self.notice = "Moira participation worker unavailable".into();
        }
    }

    fn admit(&mut self, observation: Observation) {
        self.pending = false;
        match observation {
            Observation::Current(view) => {
                self.notice = format!("Current governed revision {}", view.revision);
                self.view = Some(*view);
            }
            Observation::NotFound => {
                self.view = None;
                self.notice = "Governed participation not found".into();
            }
            Observation::Refused(refusal) => {
                self.notice = format!("Moira refused: {refusal}");
            }
            Observation::InvalidReply => self.notice = "Invalid Moira participation reply".into(),
            Observation::Transport => {
                self.notice = "Moira unavailable; participation state not inferred".into();
            }
        }
    }
}

fn observe<P: AgentParticipationServicePort>(
    client: &mut AgentParticipationReadOnlyClient<P>,
    participation: participation::ParticipationIdV1,
) -> Observation {
    match client.participation(participation) {
        Ok(participation::ReplyV1::Accepted(value)) => match *value {
            participation::OutcomeV1::Participation(Some(view)) => {
                Observation::Current(Box::new(view))
            }
            participation::OutcomeV1::Participation(None) => Observation::NotFound,
            _ => Observation::InvalidReply,
        },
        Ok(participation::ReplyV1::Refused(refusal)) => {
            Observation::Refused(format!("{refusal:?}"))
        }
        Err(_) => Observation::Transport,
    }
}

fn participation_lines(view: &participation::ParticipationViewV1) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(format!("phase: {:?}", view.phase)),
        Line::from(format!("participation revision: {}", view.revision)),
        Line::from(format!("session revision: {:?}", view.session_revision)),
        Line::from(format!("event cursor: {:?}", view.last_event_cursor)),
        Line::from(format!("attempt: {}", view.binding.attempt.attempt)),
        Line::from(format!("incarnation: {}", view.binding.attempt.incarnation)),
        Line::from(format!("persona: {:?}", view.binding.persona)),
        Line::from(format!(
            "grounding depth: {:?}",
            view.binding.grounding.depth
        )),
        Line::from(format!("active turn: {}", view.active_turn.is_some())),
        Line::from(format!(
            "pending operation: {}",
            view.pending.map(operation_kind).unwrap_or("none")
        )),
        Line::from(format!(
            "capabilities: permission={} read={} write={} terminal={}",
            view.capabilities.permissions,
            view.capabilities.filesystem_read,
            view.capabilities.filesystem_write,
            view.capabilities.terminal
        )),
        Line::from(format!(
            "session capabilities: load={} resume={} modes={} models={}",
            view.capabilities.load_session,
            view.capabilities.resume_session,
            view.capabilities.session_modes,
            view.capabilities.session_models
        )),
    ];
    if let Some(permission) = view.permission {
        lines.push(Line::from(format!(
            "permission: {} exact option(s)",
            permission.option_count
        )));
    }
    if let Some(terminal) = view.terminal {
        lines.push(Line::from(format!(
            "terminal: {:?} / {:?}",
            terminal.disposition, terminal.stop_reason
        )));
    }
    lines
}

fn operation_kind(value: participation::OperationV1) -> &'static str {
    match value {
        participation::OperationV1::Open { .. } => "open",
        participation::OperationV1::Prompt { .. } => "prompt",
        participation::OperationV1::Advance { .. } => "advance",
        participation::OperationV1::DecidePermission { .. } => "decide_permission",
        participation::OperationV1::Cancel { .. } => "cancel",
        participation::OperationV1::Close { .. } => "close",
        participation::OperationV1::Reattach { .. } => "reattach",
    }
}

//! Stable read-only presentation of one methodical deliberation.

use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

use moira_client::{DeliberationReadOnlyClient, DeliberationServicePort};
use moira_protocol::deliberation as d;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::{Input, Lifecycle};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeliberationObservation {
    Current {
        registry_revision: u64,
        view: Box<d::DeliberationViewV1>,
    },
    NotFound {
        registry_revision: u64,
    },
    Stale,
    NeedsReopen,
    Refused(String),
    InvalidReply,
    Transport,
}

pub fn observe_deliberation<P: DeliberationServicePort>(
    client: &mut DeliberationReadOnlyClient<P>,
    id: d::DeliberationIdV1,
) -> DeliberationObservation {
    let first = match read_status(client.status()) {
        Ok(value) => value,
        Err(observation) => return observation,
    };
    if first.needs_reopen {
        return DeliberationObservation::NeedsReopen;
    }
    let view = match client.deliberation(id) {
        Ok(d::ReplyV1::Accepted(d::OutcomeV1::Deliberation(value))) => value,
        Ok(d::ReplyV1::Accepted(_)) => return DeliberationObservation::InvalidReply,
        Ok(d::ReplyV1::Refused(refusal)) => {
            return DeliberationObservation::Refused(format!("{refusal:?}"));
        }
        Err(_) => return DeliberationObservation::Transport,
    };
    let second = match read_status(client.status()) {
        Ok(value) => value,
        Err(observation) => return observation,
    };
    if second.needs_reopen {
        DeliberationObservation::NeedsReopen
    } else if first.revision != second.revision {
        DeliberationObservation::Stale
    } else if let Some(view) = view {
        DeliberationObservation::Current {
            registry_revision: second.revision,
            view,
        }
    } else {
        DeliberationObservation::NotFound {
            registry_revision: second.revision,
        }
    }
}

fn read_status<E>(value: Result<d::ReplyV1, E>) -> Result<d::StatusV1, DeliberationObservation> {
    match value {
        Ok(d::ReplyV1::Accepted(d::OutcomeV1::Status(value))) => Ok(value),
        Ok(d::ReplyV1::Accepted(_)) => Err(DeliberationObservation::InvalidReply),
        Ok(d::ReplyV1::Refused(refusal)) => {
            Err(DeliberationObservation::Refused(format!("{refusal:?}")))
        }
        Err(_) => Err(DeliberationObservation::Transport),
    }
}

pub struct DeliberationComponent {
    requests: SyncSender<()>,
    replies: Receiver<DeliberationObservation>,
    pending: bool,
    observation: Option<(u64, Box<d::DeliberationViewV1>)>,
    selected: usize,
    notice: String,
}

impl DeliberationComponent {
    pub fn new<P>(port: P, id: d::DeliberationIdV1) -> Self
    where
        P: DeliberationServicePort + Send + 'static,
        P::Error: Send + 'static,
    {
        let (request_tx, request_rx) = sync_channel::<()>(1);
        let (reply_tx, reply_rx) = sync_channel::<DeliberationObservation>(1);
        std::thread::spawn(move || {
            let mut client = DeliberationReadOnlyClient::new(port);
            while request_rx.recv().is_ok() {
                if reply_tx
                    .send(observe_deliberation(&mut client, id))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut component = Self {
            requests: request_tx,
            replies: reply_rx,
            pending: false,
            observation: None,
            selected: 0,
            notice: "Methodical deliberation ready; refresh queued".into(),
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
                    self.notice = "Moira deliberation worker unavailable".into();
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
            Input::Up | Input::Back => {
                self.selected = self.selected.saturating_sub(1);
                Lifecycle::Running
            }
            Input::Down | Input::Open => {
                let length = self
                    .observation
                    .as_ref()
                    .map_or(0, |(_, view)| detail_lines(view, 0).len());
                if self.selected + 1 < length {
                    self.selected += 1;
                }
                Lifecycle::Running
            }
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
            " Moira deliberation • focused "
        } else {
            " Moira deliberation "
        };
        let lines = self.observation.as_ref().map_or_else(
            || vec![Line::from("No stable deliberation observation")],
            |(_, view)| detail_lines(view, self.selected),
        );
        let visible = usize::from(body.height.saturating_sub(2)).max(1);
        let scroll = self.selected.saturating_add(1).saturating_sub(visible);
        frame.render_widget(
            Paragraph::new(lines)
                .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0))
                .block(Block::default().title(title).borders(Borders::ALL)),
            body,
        );
        frame.render_widget(
            Paragraph::new("j/k traverses exact facts • r refreshes • q closes"),
            controls,
        );
        frame.render_widget(Paragraph::new(self.notice.as_str()), notice);
    }

    fn offer(&mut self) {
        if self.pending {
            self.notice = "Moira deliberation request already pending".into();
        } else if self.requests.try_send(()).is_ok() {
            self.pending = true;
            self.notice = "Moira deliberation refresh pending".into();
        } else {
            self.notice = "Moira deliberation worker unavailable".into();
        }
    }

    fn admit(&mut self, observation: DeliberationObservation) {
        self.pending = false;
        match observation {
            DeliberationObservation::Current {
                registry_revision,
                view,
            } => {
                let length = detail_lines(&view, 0).len();
                self.selected = self.selected.min(length.saturating_sub(1));
                self.notice = format!(
                    "Stable registry revision {registry_revision}; no frontier or authority inferred"
                );
                self.observation = Some((registry_revision, view));
            }
            DeliberationObservation::NotFound { registry_revision } => {
                self.observation = None;
                self.notice = format!("No deliberation at registry revision {registry_revision}");
            }
            DeliberationObservation::Stale => {
                self.notice = "Deliberation changed during observation; refresh required".into();
            }
            DeliberationObservation::NeedsReopen => {
                self.notice = "Deliberation store requires reopen; state not inferred".into();
            }
            DeliberationObservation::Refused(refusal) => {
                self.notice = format!("Moira refused: {refusal}");
            }
            DeliberationObservation::InvalidReply => {
                self.notice = "Invalid Moira deliberation reply; state not inferred".into();
            }
            DeliberationObservation::Transport => {
                self.notice = "Moira unavailable; deliberation state not inferred".into();
            }
        }
    }
}

fn detail_lines(view: &d::DeliberationViewV1, selected: usize) -> Vec<Line<'static>> {
    let mut values = vec![
        format!("phase: {:?}", view.phase),
        format!("remaining fuel: {}", view.remaining_fuel),
        format!("question: {}", digest(view.plan.question_digest)),
        format!(
            "method: machine={} state={}",
            digest(view.plan.method.machine_digest),
            digest(view.plan.method.state_digest)
        ),
        format!(
            "orthogonality: exact={} query={} result={}",
            view.plan.orthogonality.all_orthogonal,
            digest(view.plan.orthogonality.query_digest),
            digest(view.plan.orthogonality.result_digest)
        ),
        format!("claims: {:?}", view.plan.claims),
    ];
    for member in &view.plan.members {
        values.push(format!(
            "persona {:?}: member={} concern={} grounding={:?}/{}",
            member.lens,
            identity(member.id.0),
            digest(member.concern_root_digest),
            member.grounding.depth,
            digest(member.grounding.surface_digest)
        ));
    }
    for report in &view.reports {
        values.push(format!(
            "report {} member={} output={} evidence={} dissent={}",
            identity(report.id.0),
            identity(report.member.0),
            digest(report.output_digest),
            report.evidence_digests.len(),
            report.dissent_digests.len()
        ));
    }
    for candidate in &view.plan.candidates {
        values.push(format!(
            "candidate {} scores={:?}",
            identity(candidate.id.0),
            candidate.scores
        ));
    }
    values.push(resolution_summary(view.resolution.as_ref()));
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            Line::from(format!(
                "{} {value}",
                if index == selected { ">" } else { " " }
            ))
        })
        .collect()
}

fn resolution_summary(value: Option<&d::ResolutionV1>) -> String {
    match value {
        None => "resolution: unresolved".into(),
        Some(d::ResolutionV1::ProvenBool { value, .. }) => {
            format!("resolution: proven bool {value}")
        }
        Some(d::ResolutionV1::AuthorizedChoice {
            frontier, selected, ..
        }) => format!(
            "resolution: authorized choice selected={} retained-frontier={:?}",
            identity(selected.0),
            frontier
                .iter()
                .map(|value| identity(value.0))
                .collect::<Vec<_>>()
        ),
        Some(d::ResolutionV1::Maybe(value)) => format!("resolution: maybe {value:?}"),
        Some(d::ResolutionV1::Mu(value)) => format!("resolution: mu {value:?}"),
    }
}

fn identity(value: [u8; 16]) -> String {
    hex(&value)
}

fn digest(value: [u8; 32]) -> String {
    hex(&value)
}

fn hex(value: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(value.len() * 2);
    for byte in value {
        result.push(char::from(DIGITS[usize::from(byte >> 4)]));
        result.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    result
}

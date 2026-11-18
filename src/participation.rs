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
    prior_revision: Option<u64>,
    notice: String,
}

/// Bounded contribution to a composition host's read-only operator agenda.
/// It describes an owner-published wait; it does not grant the host permission
/// to choose or confirm an option.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParticipationAgendaObservation {
    pub revision: u64,
    pub phase: String,
    pub pending_operation: Option<&'static str>,
    pub permission_option_count: Option<u8>,
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
            prior_revision: None,
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

    pub fn operator_agenda_observation(&self) -> Option<ParticipationAgendaObservation> {
        self.view
            .as_ref()
            .map(|view| ParticipationAgendaObservation {
                revision: view.revision,
                phase: format!("{:?}", view.phase),
                pending_operation: view.pending.map(operation_kind),
                permission_option_count: view.permission.map(|value| value.option_count),
            })
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
            Input::Up | Input::Down | Input::Toggle | Input::Open | Input::Back => {
                Lifecycle::Running
            }
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
            |view| participation_lines(view, self.prior_revision),
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
                if let Some(current) = self.view.as_ref() {
                    if view.revision < current.revision {
                        self.notice = format!(
                            "Moira revision regressed {} → {}; keeping last trusted projection",
                            current.revision, view.revision
                        );
                        return;
                    }
                    if view.revision == current.revision && view.as_ref() != current {
                        self.notice = format!(
                            "Moira changed revision {} in place; keeping last trusted projection",
                            current.revision
                        );
                        return;
                    }
                    if view.revision > current.revision {
                        self.prior_revision = Some(current.revision);
                        self.notice = format!(
                            "Governed revision advanced {} → {}",
                            current.revision, view.revision
                        );
                        self.view = Some(*view);
                        return;
                    }
                }
                self.notice = format!("Current governed revision {}", view.revision);
                self.view = Some(*view);
            }
            Observation::NotFound => {
                self.view = None;
                self.prior_revision = None;
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

fn participation_lines(
    view: &participation::ParticipationViewV1,
    prior_revision: Option<u64>,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from("principal type: agent"),
        Line::from("accountable owner: Moira"),
        Line::from(format!(
            "participation: {}",
            hex_bytes(&view.binding.participation.0)
        )),
        Line::from(format!("phase: {:?}", view.phase)),
        Line::from(format!("participation revision: {}", view.revision)),
        Line::from(format!(
            "previously observed revision: {}",
            prior_revision.map_or_else(|| "none".into(), |revision| revision.to_string())
        )),
        Line::from(format!("session revision: {:?}", view.session_revision)),
        Line::from(format!("event cursor: {:?}", view.last_event_cursor)),
        Line::from(format!("workflow: {}", hex_bytes(&view.binding.workflow.0))),
        Line::from(format!(
            "deliberation: {}",
            hex_bytes(&view.binding.deliberation.0)
        )),
        Line::from(format!("member: {}", hex_bytes(&view.binding.member.0))),
        Line::from(format!("session: {}", hex_bytes(&view.binding.session.0))),
        Line::from(format!("profile: {}", hex_bytes(&view.binding.profile.0))),
        Line::from(format!(
            "task: {} · incarnation {} · attempt {}",
            hex_bytes(&view.binding.attempt.task_key),
            view.binding.attempt.incarnation,
            view.binding.attempt.attempt
        )),
        Line::from(format!("persona: {:?}", view.binding.persona)),
        Line::from(format!(
            "grounding depth: {:?}",
            view.binding.grounding.depth
        )),
        Line::from(format!(
            "grounding authority: {}",
            hex_bytes(&view.binding.grounding.authority_digest)
        )),
        Line::from(format!(
            "grounding surface: {}",
            hex_bytes(&view.binding.grounding.surface_digest)
        )),
        Line::from(format!(
            "method machine: {}",
            hex_bytes(&view.binding.method.machine_digest)
        )),
        Line::from(format!(
            "method state: {}",
            hex_bytes(&view.binding.method.state_digest)
        )),
        Line::from(format!(
            "profile document: {}",
            view.profile_document_digest
                .map(|digest| hex_bytes(&digest))
                .unwrap_or_else(|| "not published".into())
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
        Line::from(
            "not published: role/work mode · scope · delegation · budgets · authority reference · policy epoch · expiry · accountable human",
        ),
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

fn hex_bytes(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
    }
    value
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

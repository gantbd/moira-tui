//! Closed, review/arm/confirm preparation of one governed agent-session open.

use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

use moira_client::{
    AgentParticipationOpenClient, AgentParticipationReadOnlyClient, AgentParticipationServicePort,
    PrepareAgentOpenDispositionV1, PrepareAgentOpenErrorV1,
};
use moira_protocol::agent_participation as participation;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::Lifecycle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenControlInput {
    Refresh,
    Arm,
    Confirm,
    Back,
    Close,
}

#[derive(Clone, Copy)]
enum Request {
    Refresh,
    Confirm,
}

enum Observation {
    Current(Box<participation::ParticipationViewV1>),
    Prepared {
        view: Box<participation::ParticipationViewV1>,
        replayed: bool,
    },
    Refused(String),
    InvalidReply,
    ReadUnavailable,
    OutcomeUnknown,
}

pub struct OpenControlComponent {
    requests: SyncSender<Request>,
    replies: Receiver<Observation>,
    pending: bool,
    armed: bool,
    fenced: bool,
    view: Option<participation::ParticipationViewV1>,
    notice: String,
}

impl OpenControlComponent {
    pub fn new<P>(port: P, id: participation::ParticipationIdV1) -> Self
    where
        P: AgentParticipationServicePort + Send + 'static,
        P::Error: Send + 'static,
    {
        let (request_tx, request_rx) = sync_channel(1);
        let (reply_tx, reply_rx) = sync_channel(1);
        std::thread::spawn(move || {
            let mut port = Some(port);
            while let Ok(request) = request_rx.recv() {
                let observation = match request {
                    Request::Refresh => {
                        let mut client = AgentParticipationReadOnlyClient::new(
                            port.take().expect("participation port"),
                        );
                        let observation = current(&mut client, id);
                        port = Some(client.into_port());
                        observation
                    }
                    Request::Confirm => {
                        let mut client = AgentParticipationOpenClient::new(
                            port.take().expect("participation port"),
                        );
                        let observation = match client.prepare(id) {
                            Ok(PrepareAgentOpenDispositionV1::Prepared {
                                participation,
                                replayed,
                                ..
                            }) => Observation::Prepared {
                                view: Box::new(participation),
                                replayed,
                            },
                            Ok(PrepareAgentOpenDispositionV1::AlreadyPrepared {
                                participation,
                                ..
                            }) => Observation::Prepared {
                                view: Box::new(participation),
                                replayed: true,
                            },
                            Err(PrepareAgentOpenErrorV1::NotAvailable) => Observation::Refused(
                                "open is not available in current owner state".into(),
                            ),
                            Err(PrepareAgentOpenErrorV1::Refused(refusal)) => {
                                Observation::Refused(format!("{refusal:?}"))
                            }
                            Err(PrepareAgentOpenErrorV1::InvalidReply) => Observation::InvalidReply,
                            Err(PrepareAgentOpenErrorV1::Transport(_)) => {
                                Observation::OutcomeUnknown
                            }
                        };
                        port = Some(client.into_port());
                        observation
                    }
                };
                if reply_tx.send(observation).is_err() {
                    break;
                }
            }
        });
        let mut value = Self {
            requests: request_tx,
            replies: reply_rx,
            pending: false,
            armed: false,
            fenced: false,
            view: None,
            notice: "Agent-session open inspection queued".into(),
        };
        value.offer(Request::Refresh);
        value
    }

    pub fn poll(&mut self) {
        loop {
            match self.replies.try_recv() {
                Ok(observation) => self.admit(observation),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pending = false;
                    self.notice = "Moira open worker unavailable".into();
                    break;
                }
            }
        }
    }

    pub fn notice(&self) -> &str {
        &self.notice
    }

    pub const fn request_pending(&self) -> bool {
        self.pending
    }

    pub const fn recovery_required(&self) -> bool {
        self.fenced
    }

    pub const fn armed(&self) -> bool {
        self.armed
    }

    pub fn update(&mut self, input: OpenControlInput, focused: bool) -> Lifecycle {
        if !focused {
            return Lifecycle::Running;
        }
        match input {
            OpenControlInput::Close => return Lifecycle::CloseRequested,
            OpenControlInput::Refresh => {
                self.armed = false;
                self.offer(Request::Refresh);
            }
            OpenControlInput::Arm if self.can_prepare() => {
                self.armed = true;
                self.notice = "Agent-session open armed; Enter confirms, Esc disarms".into();
            }
            OpenControlInput::Confirm if self.armed && self.can_prepare() => {
                self.armed = false;
                self.offer(Request::Confirm);
            }
            OpenControlInput::Back => {
                self.armed = false;
                self.notice = "Agent-session open disarmed".into();
            }
            OpenControlInput::Arm | OpenControlInput::Confirm => {}
        }
        Lifecycle::Running
    }

    pub fn draw_in(&self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        let title = if focused {
            " Moira prepare agent session • focused "
        } else {
            " Moira prepare agent session "
        };
        let mut lines = self.view.as_ref().map_or_else(
            || vec![Line::from("No current governed participation")],
            |view| {
                vec![
                    Line::from(format!("phase: {:?}", view.phase)),
                    Line::from(format!("participation revision: {}", view.revision)),
                    Line::from(format!("persona: {:?}", view.binding.persona)),
                    Line::from(format!(
                        "task attempt: {} / {}",
                        view.binding.attempt.incarnation, view.binding.attempt.attempt
                    )),
                    Line::from(format!(
                        "task key: {}",
                        full_hex(&view.binding.attempt.task_key)
                    )),
                    Line::from(format!("session: {}", full_hex(&view.binding.session.0))),
                    Line::from(format!("profile: {}", full_hex(&view.binding.profile.0))),
                    Line::from(format!("pending operation: {:?}", view.pending)),
                ]
            },
        );
        lines.push(Line::from(if self.fenced {
            "OUTCOME UNKNOWN: refresh inspects retained owner state; do not blindly retry"
        } else if self.armed {
            "ARMED: Enter asks Moira to prepare this exact open; Esc disarms"
        } else if self.can_prepare() {
            "c arms owner-derived open • r refreshes • q closes"
        } else {
            "open cannot prepare in current state • r refreshes • q closes"
        }));
        frame.render_widget(
            Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
            body,
        );
        frame.render_widget(Paragraph::new(self.notice.as_str()), footer);
    }

    fn can_prepare(&self) -> bool {
        !self.pending
            && !self.fenced
            && self.view.as_ref().is_some_and(|view| {
                view.phase == participation::PhaseV1::Admitted && view.pending.is_none()
            })
    }

    fn offer(&mut self, request: Request) {
        if self.pending {
            self.notice = "Agent-session open request already pending".into();
        } else if self.requests.try_send(request).is_ok() {
            self.pending = true;
            self.notice = match request {
                Request::Refresh => "Agent-session open inspection pending",
                Request::Confirm => "Agent-session open preparation pending",
            }
            .into();
        } else {
            self.notice = "Moira open worker unavailable".into();
        }
    }

    fn admit(&mut self, observation: Observation) {
        self.pending = false;
        match observation {
            Observation::Current(view) => {
                self.view = Some(*view);
                self.fenced = false;
                self.notice = "Current governed participation retained for review".into();
            }
            Observation::Prepared { view, replayed } => {
                self.view = Some(*view);
                self.fenced = false;
                self.notice = if replayed {
                    "Exact agent-session open was already durably prepared"
                } else {
                    "Agent-session open durably prepared; owner worker may dispatch it"
                }
                .into();
            }
            Observation::Refused(refusal) => {
                self.notice =
                    format!("Moira refused open preparation: {refusal}; refresh required");
            }
            Observation::InvalidReply => {
                self.notice = "Invalid open-preparation reply; refresh required".into();
            }
            Observation::ReadUnavailable => {
                self.notice = "Moira unavailable; open state not inferred".into();
            }
            Observation::OutcomeUnknown => {
                self.fenced = true;
                self.notice =
                    "Open-preparation outcome unknown; refresh inspects retained state".into();
            }
        }
    }
}

fn current<P: AgentParticipationServicePort>(
    client: &mut AgentParticipationReadOnlyClient<P>,
    id: participation::ParticipationIdV1,
) -> Observation {
    match client.participation(id) {
        Ok(participation::ReplyV1::Accepted(value)) => match *value {
            participation::OutcomeV1::Participation(Some(value)) => {
                Observation::Current(Box::new(value))
            }
            participation::OutcomeV1::Participation(None) => {
                Observation::Refused("participation not found".into())
            }
            _ => Observation::InvalidReply,
        },
        Ok(participation::ReplyV1::Refused(refusal)) => {
            Observation::Refused(format!("{refusal:?}"))
        }
        Err(_) => Observation::ReadUnavailable,
    }
}

fn full_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

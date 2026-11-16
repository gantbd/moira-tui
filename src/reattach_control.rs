//! Closed, two-step reattach control for one governed participation.

use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

use moira_client::{
    AgentParticipationReadOnlyClient, AgentParticipationReattachClient,
    AgentParticipationServicePort,
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
pub enum ReattachControlInput {
    Refresh,
    Arm,
    Confirm,
    Back,
    Close,
}

#[derive(Clone, Copy)]
enum Request {
    Refresh,
    Confirm(participation::ConfirmedReattachV1),
}

enum Observation {
    Current {
        view: Box<participation::ParticipationViewV1>,
        challenge: participation::ReattachConfirmationV1,
    },
    NotRequired(Box<participation::ParticipationViewV1>),
    Applied {
        view: Box<participation::ParticipationViewV1>,
        replayed: bool,
    },
    Refused(String),
    InvalidReply,
    ReadUnavailable,
    OutcomeUnknown,
}

pub struct ReattachControlComponent {
    requests: SyncSender<Request>,
    replies: Receiver<Observation>,
    pending: bool,
    armed: bool,
    fenced: bool,
    view: Option<participation::ParticipationViewV1>,
    challenge: Option<participation::ReattachConfirmationV1>,
    notice: String,
}

impl ReattachControlComponent {
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
                        let observation = stable_snapshot(&mut client, id);
                        port = Some(client.into_port());
                        observation
                    }
                    Request::Confirm(confirmation) => {
                        let mut client = AgentParticipationReattachClient::new(
                            port.take().expect("participation port"),
                        );
                        let observation = match client.confirm(confirmation) {
                            Ok(participation::ReplyV1::Accepted(value)) => match *value {
                                participation::OutcomeV1::Applied {
                                    participation,
                                    replayed,
                                    ..
                                } => Observation::Applied {
                                    view: Box::new(participation),
                                    replayed,
                                },
                                _ => Observation::InvalidReply,
                            },
                            Ok(participation::ReplyV1::Refused(refusal)) => {
                                Observation::Refused(format!("{refusal:?}"))
                            }
                            Err(_) => Observation::OutcomeUnknown,
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
            challenge: None,
            notice: "Reattach inspection queued".into(),
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
                    self.notice = "Moira reattach worker unavailable".into();
                    break;
                }
            }
        }
    }

    pub fn update(&mut self, input: ReattachControlInput, focused: bool) -> Lifecycle {
        if !focused {
            return Lifecycle::Running;
        }
        match input {
            ReattachControlInput::Close => return Lifecycle::CloseRequested,
            ReattachControlInput::Refresh => {
                self.armed = false;
                self.offer(Request::Refresh);
            }
            ReattachControlInput::Arm
                if !self.pending && !self.fenced && self.challenge.is_some() =>
            {
                self.armed = true;
                self.notice = "Reattach armed; Enter confirms, Esc disarms".into();
            }
            ReattachControlInput::Confirm if self.armed && !self.pending && !self.fenced => {
                let challenge = self.challenge.expect("armed challenge");
                self.armed = false;
                self.offer(Request::Confirm(participation::ConfirmedReattachV1 {
                    participation: challenge.participation,
                    confirmation_digest: challenge.confirmation_digest,
                }));
            }
            ReattachControlInput::Back => {
                self.armed = false;
                self.notice = "Reattach disarmed".into();
            }
            ReattachControlInput::Arm | ReattachControlInput::Confirm => {}
        }
        Lifecycle::Running
    }

    pub fn draw_in(&self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        let title = if focused {
            " Moira governed reattach • focused "
        } else {
            " Moira governed reattach "
        };
        let mut lines = self.view.as_ref().map_or_else(
            || vec![Line::from("No current governed participation")],
            |view| {
                vec![
                    Line::from(format!("phase: {:?}", view.phase)),
                    Line::from(format!("participation revision: {}", view.revision)),
                    Line::from(format!("session revision: {:?}", view.session_revision)),
                    Line::from(format!("persona: {:?}", view.binding.persona)),
                    Line::from(format!(
                        "grounding depth: {:?}",
                        view.binding.grounding.depth
                    )),
                    Line::from(format!(
                        "resume capability: {}",
                        view.capabilities.resume_session
                    )),
                ]
            },
        );
        if let Some(challenge) = self.challenge {
            lines.push(Line::from(format!(
                "server challenge: storage {} digest {}",
                challenge.storage_revision,
                short_hex(&challenge.confirmation_digest)
            )));
        }
        lines.push(Line::from(if self.fenced {
            "OUTCOME UNKNOWN: refresh/inspect before any further action"
        } else if self.armed {
            "ARMED: Enter submits the exact server challenge; Esc disarms"
        } else if self.challenge.is_some() {
            "c arms reattach • r refreshes • q closes"
        } else {
            "reattach not required • r refreshes • q closes"
        }));
        frame.render_widget(
            Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
            body,
        );
        frame.render_widget(Paragraph::new(self.notice.as_str()), footer);
    }

    fn offer(&mut self, request: Request) {
        if self.pending {
            self.notice = "Reattach request already pending".into();
        } else if self.requests.try_send(request).is_ok() {
            self.pending = true;
            self.notice = match request {
                Request::Refresh => "Reattach inspection pending".into(),
                Request::Confirm(_) => "Reattach confirmation pending".into(),
            };
        } else {
            self.notice = "Moira reattach worker unavailable".into();
        }
    }

    fn admit(&mut self, observation: Observation) {
        self.pending = false;
        match observation {
            Observation::Current { view, challenge } => {
                self.view = Some(*view);
                self.challenge = Some(challenge);
                self.fenced = false;
                self.notice = "Current server reattach challenge retained".into();
            }
            Observation::NotRequired(view) => {
                self.view = Some(*view);
                self.challenge = None;
                self.fenced = false;
                self.notice = "Reattach not required".into();
            }
            Observation::Applied { view, replayed } => {
                self.view = Some(*view);
                self.challenge = None;
                self.fenced = false;
                self.notice = if replayed {
                    "Exact reattach confirmation replayed"
                } else {
                    "Reattach durably prepared"
                }
                .into();
            }
            Observation::Refused(refusal) => {
                self.challenge = None;
                self.notice = format!("Moira refused reattach: {refusal}; refresh required");
            }
            Observation::InvalidReply => {
                self.challenge = None;
                self.notice = "Invalid reattach reply; refresh required".into();
            }
            Observation::ReadUnavailable => {
                self.notice = "Moira unavailable; reattach state not inferred".into();
            }
            Observation::OutcomeUnknown => {
                self.challenge = None;
                self.fenced = true;
                self.notice = "Reattach outcome unknown; inspect before further action".into();
            }
        }
    }
}

fn stable_snapshot<P: AgentParticipationServicePort>(
    client: &mut AgentParticipationReadOnlyClient<P>,
    id: participation::ParticipationIdV1,
) -> Observation {
    let first = match current(client, id) {
        Ok(value) => value,
        Err(value) => return value,
    };
    let first_challenge = match challenge(client, id) {
        Ok(value) => value,
        Err(value) => return value,
    };
    let Some(first_challenge) = first_challenge else {
        return Observation::NotRequired(Box::new(first));
    };
    if participation::reattach_confirmation(first_challenge.storage_revision, &first)
        != Some(first_challenge)
    {
        return Observation::InvalidReply;
    }
    let second = match current(client, id) {
        Ok(value) => value,
        Err(value) => return value,
    };
    let second_challenge = match challenge(client, id) {
        Ok(value) => value,
        Err(value) => return value,
    };
    if first != second || second_challenge != Some(first_challenge) {
        Observation::Refused("stale read model".into())
    } else {
        Observation::Current {
            view: Box::new(first),
            challenge: first_challenge,
        }
    }
}

fn current<P: AgentParticipationServicePort>(
    client: &mut AgentParticipationReadOnlyClient<P>,
    id: participation::ParticipationIdV1,
) -> Result<participation::ParticipationViewV1, Observation> {
    match client.participation(id) {
        Ok(participation::ReplyV1::Accepted(value)) => match *value {
            participation::OutcomeV1::Participation(Some(value)) => Ok(value),
            participation::OutcomeV1::Participation(None) => {
                Err(Observation::Refused("participation not found".into()))
            }
            _ => Err(Observation::InvalidReply),
        },
        Ok(participation::ReplyV1::Refused(refusal)) => {
            Err(Observation::Refused(format!("{refusal:?}")))
        }
        Err(_) => Err(Observation::ReadUnavailable),
    }
}

fn challenge<P: AgentParticipationServicePort>(
    client: &mut AgentParticipationReadOnlyClient<P>,
    id: participation::ParticipationIdV1,
) -> Result<Option<participation::ReattachConfirmationV1>, Observation> {
    match client.reattach_confirmation(id) {
        Ok(participation::ReplyV1::Accepted(value)) => match *value {
            participation::OutcomeV1::ReattachConfirmation(value) => Ok(value),
            _ => Err(Observation::InvalidReply),
        },
        Ok(participation::ReplyV1::Refused(refusal)) => {
            Err(Observation::Refused(format!("{refusal:?}")))
        }
        Err(_) => Err(Observation::ReadUnavailable),
    }
}

fn short_hex(bytes: &[u8; 32]) -> String {
    bytes[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

//! Closed interactive submission of one bounded exact prompt.

use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

use moira_client::{
    AgentParticipationPromptClient, AgentParticipationReadOnlyClient, AgentParticipationServicePort,
};
use moira_protocol::agent_participation as participation;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};
use sha2::{Digest, Sha256};

use crate::Lifecycle;

pub const MAX_INTERACTIVE_PROMPT_BYTES: usize = 60 * 1024;
const PREVIEW_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromptControlError {
    InvalidPrompt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromptControlInput {
    Refresh,
    Arm,
    Confirm,
    Back,
    Close,
}

enum Request {
    Refresh,
    Submit(Vec<u8>),
}

enum Observation {
    Current(Box<participation::ParticipationViewV1>),
    Applied {
        view: Box<participation::ParticipationViewV1>,
        replayed: bool,
    },
    Refused(String),
    InvalidReply,
    ReadUnavailable,
    OutcomeUnknown,
}

pub struct PromptControlComponent {
    requests: SyncSender<Request>,
    replies: Receiver<Observation>,
    prompt: Vec<u8>,
    prompt_digest: [u8; 32],
    pending: bool,
    armed: bool,
    fenced: bool,
    view: Option<participation::ParticipationViewV1>,
    notice: String,
}

impl PromptControlComponent {
    pub fn try_new<P>(
        port: P,
        id: participation::ParticipationIdV1,
        prompt: Vec<u8>,
    ) -> Result<Self, PromptControlError>
    where
        P: AgentParticipationServicePort + Send + 'static,
        P::Error: Send + 'static,
    {
        if prompt.is_empty() || prompt.len() > MAX_INTERACTIVE_PROMPT_BYTES {
            return Err(PromptControlError::InvalidPrompt);
        }
        let prompt_digest = Sha256::digest(&prompt).into();
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
                        let observation = stable_current(&mut client, id);
                        port = Some(client.into_port());
                        observation
                    }
                    Request::Submit(prompt) => {
                        let mut client = AgentParticipationPromptClient::new(
                            port.take().expect("participation port"),
                        );
                        let observation = match client.submit(id, prompt) {
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
            prompt,
            prompt_digest,
            pending: false,
            armed: false,
            fenced: false,
            view: None,
            notice: "Prompt participation inspection queued".into(),
        };
        value.offer(Request::Refresh);
        Ok(value)
    }

    pub fn poll(&mut self) {
        loop {
            match self.replies.try_recv() {
                Ok(observation) => self.admit(observation),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pending = false;
                    self.notice = "Moira prompt worker unavailable".into();
                    break;
                }
            }
        }
    }

    pub fn update(&mut self, input: PromptControlInput, focused: bool) -> Lifecycle {
        if !focused {
            return Lifecycle::Running;
        }
        match input {
            PromptControlInput::Close => return Lifecycle::CloseRequested,
            PromptControlInput::Refresh => {
                self.armed = false;
                self.offer(Request::Refresh);
            }
            PromptControlInput::Arm if self.can_submit() => {
                self.armed = true;
                self.notice = "Exact prompt armed; Enter submits, Esc disarms".into();
            }
            PromptControlInput::Confirm if self.armed && self.can_submit() => {
                self.armed = false;
                self.offer(Request::Submit(self.prompt.clone()));
            }
            PromptControlInput::Back => {
                self.armed = false;
                self.notice = "Prompt submission disarmed".into();
            }
            PromptControlInput::Arm | PromptControlInput::Confirm => {}
        }
        Lifecycle::Running
    }

    pub fn draw_in(&self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        let title = if focused {
            " Moira governed prompt • focused "
        } else {
            " Moira governed prompt "
        };
        let mut lines = self.view.as_ref().map_or_else(
            || vec![Line::from("No current governed participation")],
            |view| {
                vec![
                    Line::from(format!("phase: {:?}", view.phase)),
                    Line::from(format!("participation revision: {}", view.revision)),
                    Line::from(format!("session revision: {:?}", view.session_revision)),
                    Line::from(format!("attempt: {}", view.binding.attempt.attempt)),
                    Line::from(format!("persona: {:?}", view.binding.persona)),
                    Line::from(format!(
                        "grounding depth: {:?}",
                        view.binding.grounding.depth
                    )),
                    Line::from(format!("pending operation: {}", view.pending.is_some())),
                ]
            },
        );
        lines.push(Line::from(format!("prompt bytes: {}", self.prompt.len())));
        lines.push(Line::from(format!(
            "prompt SHA-256: {}",
            full_hex(&self.prompt_digest)
        )));
        let preview_length = self.prompt.len().min(PREVIEW_BYTES);
        let suffix = if preview_length < self.prompt.len() {
            " …[bounded preview]"
        } else {
            ""
        };
        lines.push(Line::from(format!(
            "prompt preview: {}{suffix}",
            display_bytes(&self.prompt[..preview_length])
        )));
        lines.push(Line::from(if self.fenced {
            "OUTCOME UNKNOWN: refresh/inspect before any further action"
        } else if self.armed {
            "ARMED: Enter submits these exact bytes; Esc disarms"
        } else if self.can_submit() {
            "c arms prompt • r refreshes • q closes"
        } else {
            "prompt cannot submit in current state • r refreshes • q closes"
        }));
        frame.render_widget(
            Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
            body,
        );
        frame.render_widget(Paragraph::new(self.notice.as_str()), footer);
    }

    fn can_submit(&self) -> bool {
        !self.pending
            && !self.fenced
            && self.view.as_ref().is_some_and(|view| {
                view.phase == participation::PhaseV1::Ready
                    && view.pending.is_none()
                    && view.session_revision.is_some()
            })
    }

    fn offer(&mut self, request: Request) {
        if self.pending {
            self.notice = "Prompt request already pending".into();
        } else if self.requests.try_send(request).is_ok() {
            self.pending = true;
            self.notice = "Prompt request pending".into();
        } else {
            self.notice = "Moira prompt worker unavailable".into();
        }
    }

    fn admit(&mut self, observation: Observation) {
        self.pending = false;
        match observation {
            Observation::Current(view) => {
                self.view = Some(*view);
                self.fenced = false;
                self.notice = "Stable governed participation retained".into();
            }
            Observation::Applied { view, replayed } => {
                self.view = Some(*view);
                self.fenced = false;
                self.notice = if replayed {
                    "Exact prompt submission replayed"
                } else {
                    "Prompt durably accepted"
                }
                .into();
            }
            Observation::Refused(refusal) => {
                self.notice = format!("Moira refused prompt: {refusal}; refresh required");
            }
            Observation::InvalidReply => {
                self.notice = "Invalid prompt reply; refresh required".into();
            }
            Observation::ReadUnavailable => {
                self.notice = "Moira unavailable; prompt state not inferred".into();
            }
            Observation::OutcomeUnknown => {
                self.fenced = true;
                self.notice = "Prompt outcome unknown; inspect before further action".into();
            }
        }
    }
}

fn stable_current<P: AgentParticipationServicePort>(
    client: &mut AgentParticipationReadOnlyClient<P>,
    id: participation::ParticipationIdV1,
) -> Observation {
    let first = match current(client, id) {
        Ok(value) => value,
        Err(value) => return value,
    };
    let second = match current(client, id) {
        Ok(value) => value,
        Err(value) => return value,
    };
    if first == second {
        Observation::Current(Box::new(first))
    } else {
        Observation::Refused("stale read model".into())
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

fn display_bytes(bytes: &[u8]) -> String {
    let mut output = String::new();
    for byte in bytes {
        if byte.is_ascii_graphic() && *byte != b'\\' || *byte == b' ' {
            output.push(char::from(*byte));
        } else {
            output.push_str(&format!("\\x{byte:02x}"));
        }
    }
    output
}

fn full_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

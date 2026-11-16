//! Closed interactive selection of one exactly correlated permission option.

use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

use moira_client::{
    AgentParticipationPermissionClient, AgentParticipationReadOnlyClient,
    AgentParticipationServicePort, AgentReadOnlyClient, AgentServicePort,
};
use moira_protocol::{agent, agent_participation as participation};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};
use sha2::{Digest, Sha256};

use crate::Lifecycle;

const PAGE_LIMIT: u64 = 64;
const MAX_PAGES: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionControlInput {
    Up,
    Down,
    Refresh,
    Arm,
    Confirm,
    Back,
    Close,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Request {
    Refresh,
    Confirm(participation::ConfirmedPermissionSelectionV1),
}

struct Snapshot {
    view: participation::ParticipationViewV1,
    confirmation: participation::PermissionConfirmationV1,
    request: agent::PermissionRequestV1,
}

enum Observation {
    Current(Box<Snapshot>),
    NotAwaiting(Box<participation::ParticipationViewV1>),
    Applied {
        view: Box<participation::ParticipationViewV1>,
        replayed: bool,
    },
    Refused(String),
    InvalidReply,
    ReadUnavailable,
    OutcomeUnknown,
}

pub struct PermissionControlComponent {
    requests: SyncSender<Request>,
    replies: Receiver<Observation>,
    pending: bool,
    armed: bool,
    fenced: bool,
    selected: usize,
    snapshot: Option<Snapshot>,
    view: Option<participation::ParticipationViewV1>,
    notice: String,
}

impl PermissionControlComponent {
    pub fn new<P, A>(
        participation_port: P,
        agent_read_port: A,
        id: participation::ParticipationIdV1,
    ) -> Self
    where
        P: AgentParticipationServicePort + Send + 'static,
        P::Error: Send + 'static,
        A: AgentServicePort + Send + 'static,
        A::Error: Send + 'static,
    {
        let (request_tx, request_rx) = sync_channel(1);
        let (reply_tx, reply_rx) = sync_channel(1);
        std::thread::spawn(move || {
            let mut participation_port = Some(participation_port);
            let mut agents = AgentReadOnlyClient::new(agent_read_port);
            while let Ok(request) = request_rx.recv() {
                let observation = match request {
                    Request::Refresh => {
                        let mut client = AgentParticipationReadOnlyClient::new(
                            participation_port.take().expect("participation port"),
                        );
                        let observation = stable_snapshot(&mut client, &mut agents, id);
                        participation_port = Some(client.into_port());
                        observation
                    }
                    Request::Confirm(selection) => {
                        let mut client = AgentParticipationPermissionClient::new(
                            participation_port.take().expect("participation port"),
                        );
                        let observation = match client.confirm(selection) {
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
                        participation_port = Some(client.into_port());
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
            selected: 0,
            snapshot: None,
            view: None,
            notice: "Permission inspection queued".into(),
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
                    self.notice = "Moira permission worker unavailable".into();
                    break;
                }
            }
        }
    }

    pub fn update(&mut self, input: PermissionControlInput, focused: bool) -> Lifecycle {
        if !focused {
            return Lifecycle::Running;
        }
        match input {
            PermissionControlInput::Close => return Lifecycle::CloseRequested,
            PermissionControlInput::Refresh => {
                self.armed = false;
                self.offer(Request::Refresh);
            }
            PermissionControlInput::Up if !self.armed => self.move_selection(false),
            PermissionControlInput::Down if !self.armed => self.move_selection(true),
            PermissionControlInput::Arm
                if !self.pending && !self.fenced && self.snapshot.is_some() =>
            {
                self.armed = true;
                self.notice = "Permission choice armed; Enter confirms, Esc disarms".into();
            }
            PermissionControlInput::Confirm if self.armed && !self.pending && !self.fenced => {
                let snapshot = self.snapshot.as_ref().expect("armed snapshot");
                let option = snapshot.request.options[self.selected].id;
                let confirmation = snapshot.confirmation;
                self.armed = false;
                self.offer(Request::Confirm(
                    participation::ConfirmedPermissionSelectionV1 {
                        participation: confirmation.participation,
                        confirmation_digest: confirmation.confirmation_digest,
                        option: participation::PermissionOptionIdV1(option.0),
                    },
                ));
            }
            PermissionControlInput::Back => {
                self.armed = false;
                self.notice = "Permission choice disarmed".into();
            }
            PermissionControlInput::Up
            | PermissionControlInput::Down
            | PermissionControlInput::Arm
            | PermissionControlInput::Confirm => {}
        }
        Lifecycle::Running
    }

    pub fn draw_in(&self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        let title = if focused {
            " Moira governed permission • focused "
        } else {
            " Moira governed permission "
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
                ]
            },
        );
        if let Some(snapshot) = &self.snapshot {
            lines.push(Line::from(format!(
                "capability bytes: {}",
                display_bytes(&snapshot.request.capability)
            )));
            lines.push(Line::from(format!(
                "description bytes: {}",
                display_bytes(&snapshot.request.description)
            )));
            lines.push(Line::from(format!(
                "server challenge: storage {} digest {}",
                snapshot.confirmation.storage_revision,
                short_hex(&snapshot.confirmation.confirmation_digest)
            )));
            for (index, option) in snapshot.request.options.iter().enumerate() {
                let marker = if index == self.selected { ">" } else { " " };
                lines.push(Line::from(format!(
                    "{marker} {:?} {} [{}]",
                    option.kind,
                    display_bytes(&option.label),
                    short_id(&option.id.0)
                )));
            }
        }
        lines.push(Line::from(if self.fenced {
            "OUTCOME UNKNOWN: refresh/inspect before any further action"
        } else if self.armed {
            "ARMED: Enter submits this exact option/challenge; Esc disarms"
        } else if self.snapshot.is_some() {
            "j/k selects • c arms • r refreshes • q closes"
        } else {
            "no presentable permission request • r refreshes • q closes"
        }));
        frame.render_widget(
            Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
            body,
        );
        frame.render_widget(Paragraph::new(self.notice.as_str()), footer);
    }

    fn move_selection(&mut self, down: bool) {
        let Some(length) = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.request.options.len())
        else {
            return;
        };
        if length == 0 {
            return;
        }
        self.selected = if down {
            (self.selected + 1).min(length - 1)
        } else {
            self.selected.saturating_sub(1)
        };
    }

    fn offer(&mut self, request: Request) {
        if self.pending {
            self.notice = "Permission request already pending".into();
        } else if self.requests.try_send(request).is_ok() {
            self.pending = true;
            self.notice = match request {
                Request::Refresh => "Permission inspection pending".into(),
                Request::Confirm(_) => "Permission confirmation pending".into(),
            };
        } else {
            self.notice = "Moira permission worker unavailable".into();
        }
    }

    fn admit(&mut self, observation: Observation) {
        self.pending = false;
        match observation {
            Observation::Current(snapshot) => {
                self.view = Some(snapshot.view.clone());
                self.snapshot = Some(*snapshot);
                self.selected = 0;
                self.fenced = false;
                self.notice = "Exact current permission request retained".into();
            }
            Observation::NotAwaiting(view) => {
                self.view = Some(*view);
                self.snapshot = None;
                self.selected = 0;
                self.fenced = false;
                self.notice = "Participation is not awaiting permission".into();
            }
            Observation::Applied { view, replayed } => {
                self.view = Some(*view);
                self.snapshot = None;
                self.selected = 0;
                self.fenced = false;
                self.notice = if replayed {
                    "Exact permission confirmation replayed"
                } else {
                    "Permission choice durably accepted"
                }
                .into();
            }
            Observation::Refused(refusal) => {
                self.snapshot = None;
                self.notice = format!("Moira refused permission: {refusal}; refresh required");
            }
            Observation::InvalidReply => {
                self.snapshot = None;
                self.notice = "Invalid permission evidence; refresh required".into();
            }
            Observation::ReadUnavailable => {
                self.notice = "Moira unavailable; permission state not inferred".into();
            }
            Observation::OutcomeUnknown => {
                self.snapshot = None;
                self.fenced = true;
                self.notice = "Permission outcome unknown; inspect before further action".into();
            }
        }
    }
}

fn stable_snapshot<P: AgentParticipationServicePort, A: AgentServicePort>(
    participations: &mut AgentParticipationReadOnlyClient<P>,
    agents: &mut AgentReadOnlyClient<A>,
    id: participation::ParticipationIdV1,
) -> Observation {
    let first = match current(participations, id) {
        Ok(value) => value,
        Err(value) => return value,
    };
    if first.phase != participation::PhaseV1::AwaitingPermission || first.permission.is_none() {
        return Observation::NotAwaiting(Box::new(first));
    }
    let first_confirmation = match confirmation(participations, id) {
        Ok(Some(value)) => value,
        Ok(None) => return Observation::NotAwaiting(Box::new(first)),
        Err(value) => return value,
    };
    if participation::permission_confirmation(first_confirmation.storage_revision, &first)
        != Some(first_confirmation)
    {
        return Observation::InvalidReply;
    }
    let session = agent::SessionIdV1(first.binding.session.0);
    let before = match inspect(agents, session) {
        Ok(value) => value,
        Err(value) => return value,
    };
    let records = match complete_events(agents, session) {
        Ok(value) => value,
        Err(value) => return value,
    };
    let after = match inspect(agents, session) {
        Ok(value) => value,
        Err(value) => return value,
    };
    let second = match current(participations, id) {
        Ok(value) => value,
        Err(value) => return value,
    };
    let second_confirmation = match confirmation(participations, id) {
        Ok(value) => value,
        Err(value) => return value,
    };
    if first != second || before != after || second_confirmation != Some(first_confirmation) {
        return Observation::Refused("stale read model".into());
    }
    let Some(request) = validated_permission_request(&first, &before, &records) else {
        return Observation::InvalidReply;
    };
    Observation::Current(Box::new(Snapshot {
        view: first,
        confirmation: first_confirmation,
        request,
    }))
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

fn confirmation<P: AgentParticipationServicePort>(
    client: &mut AgentParticipationReadOnlyClient<P>,
    id: participation::ParticipationIdV1,
) -> Result<Option<participation::PermissionConfirmationV1>, Observation> {
    match client.permission_confirmation(id) {
        Ok(participation::ReplyV1::Accepted(value)) => match *value {
            participation::OutcomeV1::PermissionConfirmation(value) => Ok(value),
            _ => Err(Observation::InvalidReply),
        },
        Ok(participation::ReplyV1::Refused(refusal)) => {
            Err(Observation::Refused(format!("{refusal:?}")))
        }
        Err(_) => Err(Observation::ReadUnavailable),
    }
}

fn inspect<A: AgentServicePort>(
    client: &mut AgentReadOnlyClient<A>,
    session: agent::SessionIdV1,
) -> Result<agent::SessionViewV1, Observation> {
    match client.inspect(session) {
        Ok(agent::ReplyV1::Accepted(agent::OutcomeV1::Session(value))) => Ok(value),
        Ok(agent::ReplyV1::Accepted(_)) => Err(Observation::InvalidReply),
        Ok(agent::ReplyV1::Refused(refusal)) => Err(Observation::Refused(format!("{refusal:?}"))),
        Err(_) => Err(Observation::ReadUnavailable),
    }
}

fn complete_events<A: AgentServicePort>(
    client: &mut AgentReadOnlyClient<A>,
    session: agent::SessionIdV1,
) -> Result<Vec<agent::EventRecordV1>, Observation> {
    let mut after = None;
    let mut previous = None;
    let mut records = Vec::new();
    for _ in 0..MAX_PAGES {
        let page = match client.events(session, after, PAGE_LIMIT) {
            Ok(agent::ReplyV1::Accepted(agent::OutcomeV1::Events(value))) => value,
            Ok(agent::ReplyV1::Accepted(_)) => return Err(Observation::InvalidReply),
            Ok(agent::ReplyV1::Refused(refusal)) => {
                return Err(Observation::Refused(format!("{refusal:?}")));
            }
            Err(_) => return Err(Observation::ReadUnavailable),
        };
        if page.session != session {
            return Err(Observation::InvalidReply);
        }
        for record in page.entries {
            let expected = previous.map_or(Some(0), |cursor: u64| cursor.checked_add(1));
            if Some(record.cursor) != expected {
                return Err(Observation::InvalidReply);
            }
            previous = Some(record.cursor);
            records.push(record);
        }
        match page.next_after {
            Some(cursor) if Some(cursor) == previous => after = Some(cursor),
            Some(_) => return Err(Observation::InvalidReply),
            None => return Ok(records),
        }
    }
    Err(Observation::InvalidReply)
}

pub fn validated_permission_request(
    participation: &participation::ParticipationViewV1,
    session: &agent::SessionViewV1,
    records: &[agent::EventRecordV1],
) -> Option<agent::PermissionRequestV1> {
    let evidence = participation.permission?;
    if session.session.0 != participation.binding.session.0
        || session.phase != agent::PhaseV1::AwaitingPermission
        || Some(session.revision) != participation.session_revision
        || session.active_turn.map(|value| value.0) != Some(evidence.turn.0)
        || session.last_event_cursor != participation.last_event_cursor
        || records.last().map(|record| record.cursor) != session.last_event_cursor
        || Some(session.profile_document_digest.0) != participation.profile_document_digest
        || participation.active_turn != Some(evidence.turn)
    {
        return None;
    }
    let mut matches = records.iter().filter_map(|record| match &record.event {
        agent::EventV1::Permission(value)
            if value.request.0 == evidence.request.0 && value.turn.0 == evidence.turn.0 =>
        {
            Some(value)
        }
        _ => None,
    });
    let request = matches.next()?;
    let count = usize::from(evidence.option_count);
    if matches.next().is_some()
        || count == 0
        || count > participation::MAX_PERMISSION_OPTIONS_V1
        || request.options.len() != count
        || <[u8; 32]>::from(Sha256::digest(&request.capability)) != evidence.capability_digest
        || permission_options_digest(&request.options) != evidence.options_digest
        || request
            .options
            .iter()
            .enumerate()
            .any(|(index, option)| evidence.option_ids[index].0 != option.id.0)
        || evidence.option_ids[request.options.len()..]
            .iter()
            .any(|option| option.0 != [0; 16])
    {
        return None;
    }
    Some(request.clone())
}

pub fn permission_options_digest(values: &[agent::PermissionOptionV1]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"MOIRAOPT1");
    hash.update((values.len() as u64).to_be_bytes());
    for value in values {
        hash.update(value.id.0);
        hash.update([match value.kind {
            agent::PermissionOptionKindV1::AllowOnce => 0,
            agent::PermissionOptionKindV1::AllowAlways => 1,
            agent::PermissionOptionKindV1::RejectOnce => 2,
            agent::PermissionOptionKindV1::RejectAlways => 3,
        }]);
        hash.update((value.label.len() as u64).to_be_bytes());
        hash.update(&value.label);
    }
    hash.finalize().into()
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

fn short_hex(bytes: &[u8; 32]) -> String {
    short_id(&bytes[..8])
}

fn short_id(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

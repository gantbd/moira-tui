//! Closed, two-step reconciliation control for one exact saga proposal.

use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

use moira_client::{SagaConfirmedReconciliationClient, SagaReadOnlyClient, SagaServicePort};
use moira_protocol::{
    ConfirmedReconciliationV1, OperatorActionIdV1, OperatorReceiptSummaryV1,
    ReconciliationConfirmationV1, ReconciliationProposalV1, SagaOutcomeV1,
    SagaReconciliationViewV1, SagaReplyV1,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::Lifecycle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconciliationControlInput {
    Refresh,
    Arm,
    Confirm,
    Back,
    Close,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Request {
    Refresh(Option<OperatorActionIdV1>),
    Confirm(ConfirmedReconciliationV1),
}

pub(crate) struct Snapshot {
    pub(crate) view: SagaReconciliationViewV1,
    pub(crate) confirmation: ReconciliationConfirmationV1,
}

pub(crate) enum Observation {
    Current(Box<Snapshot>),
    Recovered(OperatorReceiptSummaryV1),
    Applied { replayed: bool },
    NotAvailable,
    Refused(String),
    InvalidReply,
    ReadUnavailable,
    OutcomeUnknown(OperatorActionIdV1),
}

pub struct ReconciliationControlComponent {
    requests: SyncSender<Request>,
    replies: Receiver<Observation>,
    pending: bool,
    armed: bool,
    fenced_action: Option<OperatorActionIdV1>,
    proposal: ReconciliationProposalV1,
    view: Option<SagaReconciliationViewV1>,
    confirmation: Option<ReconciliationConfirmationV1>,
    receipt: Option<OperatorReceiptSummaryV1>,
    notice: String,
}

impl ReconciliationControlComponent {
    pub fn new<P>(port: P, proposal: ReconciliationProposalV1) -> Self
    where
        P: SagaServicePort + Send + 'static,
        P::Error: Send + 'static,
    {
        let (request_tx, request_rx) = sync_channel(1);
        let (reply_tx, reply_rx) = sync_channel(1);
        std::thread::spawn(move || worker_loop(port, proposal, None, request_rx, reply_tx));
        Self::from_channels(request_tx, reply_rx, proposal)
    }

    /// Opens the workflow without blocking the host update thread; transport
    /// establishment and the initial stable snapshot run on the worker.
    pub fn new_with_connector<P, F, E>(connector: F, proposal: ReconciliationProposalV1) -> Self
    where
        P: SagaServicePort + Send + 'static,
        P::Error: Send + 'static,
        F: FnOnce() -> Result<P, E> + Send + 'static,
        E: Send + 'static,
    {
        let (request_tx, request_rx) = sync_channel(1);
        let (reply_tx, reply_rx) = sync_channel(1);
        std::thread::spawn(move || {
            let Ok(first_request) = request_rx.recv() else {
                return;
            };
            match connector() {
                Ok(port) => worker_loop(port, proposal, Some(first_request), request_rx, reply_tx),
                Err(_) => {
                    let _ = reply_tx.send(Observation::ReadUnavailable);
                }
            }
        });
        Self::from_channels(request_tx, reply_rx, proposal)
    }

    fn from_channels(
        request_tx: SyncSender<Request>,
        reply_rx: Receiver<Observation>,
        proposal: ReconciliationProposalV1,
    ) -> Self {
        let mut value = Self {
            requests: request_tx,
            replies: reply_rx,
            pending: false,
            armed: false,
            fenced_action: None,
            proposal,
            view: None,
            confirmation: None,
            receipt: None,
            notice: "Reconciliation inspection queued".into(),
        };
        value.offer(Request::Refresh(None));
        value
    }

    pub fn poll(&mut self) {
        loop {
            match self.replies.try_recv() {
                Ok(observation) => self.admit(observation),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pending = false;
                    self.notice = "Moira reconciliation worker unavailable".into();
                    break;
                }
            }
        }
    }

    /// Current peer-owned workflow status for host chrome and activity logs.
    /// It is presentation evidence only and never authorizes another effect.
    pub fn notice(&self) -> &str {
        &self.notice
    }

    pub const fn request_pending(&self) -> bool {
        self.pending
    }

    pub const fn recovery_required(&self) -> bool {
        self.fenced_action.is_some()
    }

    pub fn update(&mut self, input: ReconciliationControlInput, focused: bool) -> Lifecycle {
        if !focused {
            return Lifecycle::Running;
        }
        match input {
            ReconciliationControlInput::Close if self.pending || self.fenced_action.is_some() => {
                self.notice =
                    "Reconciliation is unresolved; wait or recover the exact receipt first".into();
            }
            ReconciliationControlInput::Close => return Lifecycle::CloseRequested,
            ReconciliationControlInput::Refresh => {
                self.armed = false;
                self.offer(Request::Refresh(self.fenced_action));
            }
            ReconciliationControlInput::Arm
                if !self.pending && self.fenced_action.is_none() && self.confirmation.is_some() =>
            {
                self.armed = true;
                self.notice = "Reconciliation armed; Enter confirms, Esc disarms".into();
            }
            ReconciliationControlInput::Confirm
                if self.armed && !self.pending && self.fenced_action.is_none() =>
            {
                let value = self.confirmation.expect("armed confirmation");
                self.armed = false;
                self.offer(Request::Confirm(ConfirmedReconciliationV1 {
                    storage_revision: value.storage_revision,
                    saga: value.saga,
                    disposition: value.disposition,
                    action: value.action,
                    confirmation_digest: value.confirmation_digest,
                }));
            }
            ReconciliationControlInput::Back => {
                self.armed = false;
                self.notice = "Reconciliation disarmed".into();
            }
            ReconciliationControlInput::Arm | ReconciliationControlInput::Confirm => {}
        }
        Lifecycle::Running
    }

    pub fn draw_in(&self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        let title = if focused {
            " Moira saga reconciliation • focused "
        } else {
            " Moira saga reconciliation "
        };
        let mut lines = vec![
            Line::from(format!("saga: {}", self.proposal.saga.0)),
            Line::from(format!(
                "proposed disposition: {:?}",
                self.proposal.disposition
            )),
        ];
        if let Some(view) = self.view {
            lines.extend([
                Line::from(format!("phase: {:?}", view.saga.phase)),
                Line::from(format!("automatic progress: {:?}", view.automatic_progress)),
                Line::from(format!("retained evidence: {}", view.retained_evidence)),
                Line::from(format!("fence: {:?}", view.fence)),
            ]);
        }
        if let Some(value) = self.confirmation {
            lines.push(Line::from(format!(
                "server challenge: revision {} action {} digest {}",
                value.storage_revision,
                short_hex(&value.action.0),
                short_hex(&value.confirmation_digest)
            )));
        }
        if let Some(receipt) = self.receipt {
            lines.push(Line::from(format!(
                "durable receipt: action {} authority {}/{} phase {:?}",
                short_hex(&receipt.action.0),
                receipt.authority_decision,
                receipt.authority_epoch,
                receipt.phase
            )));
        }
        lines.push(Line::from(if self.fenced_action.is_some() {
            "OUTCOME UNKNOWN: r inspects the exact durable receipt before further action"
        } else if self.armed {
            "ARMED: Enter submits the exact server challenge; Esc disarms"
        } else if self.confirmation.is_some() {
            "c arms reconciliation • r refreshes • q closes"
        } else {
            "no current reconciliation challenge • r refreshes • q closes"
        }));
        frame.render_widget(
            Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
            body,
        );
        frame.render_widget(Paragraph::new(self.notice.as_str()), footer);
    }

    fn offer(&mut self, request: Request) {
        if self.pending {
            self.notice = "Reconciliation request already pending".into();
        } else if self.requests.try_send(request).is_ok() {
            self.pending = true;
            self.notice = match request {
                Request::Refresh(Some(_)) => "Receipt recovery pending".into(),
                Request::Refresh(None) => "Reconciliation inspection pending".into(),
                Request::Confirm(_) => "Reconciliation confirmation pending".into(),
            };
        } else {
            self.notice = "Moira reconciliation worker unavailable".into();
        }
    }

    fn admit(&mut self, observation: Observation) {
        self.pending = false;
        match observation {
            Observation::Current(snapshot) => {
                self.view = Some(snapshot.view);
                self.confirmation = Some(snapshot.confirmation);
                self.receipt = None;
                self.fenced_action = None;
                self.notice = "Exact current reconciliation challenge retained".into();
            }
            Observation::Recovered(receipt) => {
                self.confirmation = None;
                self.receipt = Some(receipt);
                self.fenced_action = None;
                self.notice = "Unknown outcome recovered from durable operator receipt".into();
            }
            Observation::Applied { replayed } => {
                self.confirmation = None;
                self.fenced_action = None;
                self.notice = if replayed {
                    "Exact reconciliation replayed"
                } else {
                    "Reconciliation durably accepted"
                }
                .into();
            }
            Observation::NotAvailable => {
                self.confirmation = None;
                self.fenced_action = None;
                self.notice = "No current reconciliation challenge".into();
            }
            Observation::Refused(refusal) => {
                self.confirmation = None;
                self.notice = format!("Moira refused reconciliation: {refusal}; refresh required");
            }
            Observation::InvalidReply => {
                self.confirmation = None;
                self.notice = "Invalid reconciliation evidence; refresh required".into();
            }
            Observation::ReadUnavailable => {
                self.notice = "Moira unavailable; reconciliation state not inferred".into();
            }
            Observation::OutcomeUnknown(action) => {
                self.confirmation = None;
                self.fenced_action = Some(action);
                self.notice = "Reconciliation outcome unknown; receipt inspection required".into();
            }
        }
    }
}

fn worker_loop<P: SagaServicePort>(
    port: P,
    proposal: ReconciliationProposalV1,
    mut first_request: Option<Request>,
    request_rx: Receiver<Request>,
    reply_tx: SyncSender<Observation>,
) {
    let mut port = Some(port);
    loop {
        let request = if let Some(request) = first_request.take() {
            request
        } else if let Ok(request) = request_rx.recv() {
            request
        } else {
            break;
        };
        let observation = match request {
            Request::Refresh(recovery) => {
                let mut client = SagaReadOnlyClient::new(port.take().expect("saga port"));
                let observation = stable_snapshot(&mut client, proposal, recovery);
                port = Some(client.into_port());
                observation
            }
            Request::Confirm(confirmation) => {
                let action = confirmation.action;
                let mut client =
                    SagaConfirmedReconciliationClient::new(port.take().expect("saga port"));
                let observation = match client.confirm(confirmation) {
                    Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Applied { replayed, .. })) => {
                        Observation::Applied { replayed }
                    }
                    Ok(SagaReplyV1::Refused(refusal)) => {
                        Observation::Refused(format!("{refusal:?}"))
                    }
                    Ok(_) => Observation::InvalidReply,
                    Err(_) => Observation::OutcomeUnknown(action),
                };
                port = Some(client.into_port());
                observation
            }
        };
        if reply_tx.send(observation).is_err() {
            break;
        }
    }
}

pub(crate) fn stable_snapshot<P: SagaServicePort>(
    client: &mut SagaReadOnlyClient<P>,
    proposal: ReconciliationProposalV1,
    recovery: Option<OperatorActionIdV1>,
) -> Observation {
    if let Some(action) = recovery {
        match client.operator_receipt(action) {
            Ok(SagaReplyV1::Accepted(SagaOutcomeV1::OperatorReceipt(Some(receipt))))
                if receipt.action == action && receipt.saga == proposal.saga =>
            {
                return Observation::Recovered(receipt);
            }
            Ok(SagaReplyV1::Accepted(SagaOutcomeV1::OperatorReceipt(None))) => {}
            Ok(SagaReplyV1::Refused(refusal)) => {
                return Observation::Refused(format!("{refusal:?}"));
            }
            Ok(_) => return Observation::InvalidReply,
            Err(_) => return Observation::ReadUnavailable,
        }
    }
    let first_status = match status(client) {
        Ok(value) => value,
        Err(value) => return value,
    };
    let view = match reconciliation(client, proposal.saga) {
        Ok(Some(value)) => value,
        Ok(None) => return Observation::NotAvailable,
        Err(value) => return value,
    };
    let confirmation = match challenge(client, proposal) {
        Ok(Some(value)) => value,
        Ok(None) => return Observation::NotAvailable,
        Err(value) => return value,
    };
    let second_status = match status(client) {
        Ok(value) => value,
        Err(value) => return value,
    };
    let second_confirmation = match challenge(client, proposal) {
        Ok(value) => value,
        Err(value) => return value,
    };
    if first_status != second_status
        || first_status.needs_reopen
        || first_status.revision != confirmation.storage_revision
        || second_confirmation != Some(confirmation)
        || moira_protocol::reconciliation_confirmation(first_status.revision, view, proposal)
            != Some(confirmation)
    {
        Observation::Refused("stale read model".into())
    } else {
        Observation::Current(Box::new(Snapshot { view, confirmation }))
    }
}

fn status<P: SagaServicePort>(
    client: &mut SagaReadOnlyClient<P>,
) -> Result<moira_protocol::SagaStatusV1, Observation> {
    match client.status() {
        Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Status(value))) => Ok(value),
        Ok(SagaReplyV1::Refused(refusal)) => Err(Observation::Refused(format!("{refusal:?}"))),
        Ok(_) => Err(Observation::InvalidReply),
        Err(_) => Err(Observation::ReadUnavailable),
    }
}

fn reconciliation<P: SagaServicePort>(
    client: &mut SagaReadOnlyClient<P>,
    saga: moira_protocol::SagaIdV1,
) -> Result<Option<SagaReconciliationViewV1>, Observation> {
    match client.reconciliation(saga) {
        Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Reconciliation(value))) => Ok(value),
        Ok(SagaReplyV1::Refused(refusal)) => Err(Observation::Refused(format!("{refusal:?}"))),
        Ok(_) => Err(Observation::InvalidReply),
        Err(_) => Err(Observation::ReadUnavailable),
    }
}

fn challenge<P: SagaServicePort>(
    client: &mut SagaReadOnlyClient<P>,
    proposal: ReconciliationProposalV1,
) -> Result<Option<ReconciliationConfirmationV1>, Observation> {
    match client.reconciliation_confirmation(proposal) {
        Ok(SagaReplyV1::Accepted(SagaOutcomeV1::ReconciliationConfirmation(value))) => Ok(value),
        Ok(SagaReplyV1::Refused(refusal)) => Err(Observation::Refused(format!("{refusal:?}"))),
        Ok(_) => Err(Observation::InvalidReply),
        Err(_) => Err(Observation::ReadUnavailable),
    }
}

fn short_hex(bytes: &[u8]) -> String {
    bytes[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

//! Bounded parent workflow for independently receipted saga reconciliations.

use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

use moira_client::{SagaConfirmedReconciliationClient, SagaReadOnlyClient, SagaServicePort};
use moira_protocol::{
    ConfirmedReconciliationV1, OperatorActionIdV1, OperatorDispositionV1, OperatorReceiptSummaryV1,
    ReconciliationConfirmationV1, ReconciliationProposalV1, SagaEffectOutcomeV1, SagaOutcomeV1,
    SagaReplyV1, SagaViewV1,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::{
    Lifecycle, reconciliation_control::Observation, reconciliation_control::stable_snapshot,
};

pub const MAX_RECONCILIATION_BATCH: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconciliationBatchInput {
    Digit(char),
    Backspace,
    Prepare,
    Refresh,
    Arm,
    Confirm,
    Back,
    Close,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimedReconciliationKind {
    Cancel,
    Compensate,
    AdmitFailed,
    AdmitSucceeded,
    Retry,
}

struct TimedDraft {
    kind: TimedReconciliationKind,
    sagas: Vec<SagaViewV1>,
    logical_time: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconciliationBatchDisposition {
    Inspecting,
    Ready,
    Armed,
    Pending,
    Applied,
    Partial,
    ReceiptPending,
    OutcomeUnknown,
}

enum ChildState {
    Inspecting,
    Ready(ReconciliationConfirmationV1),
    Pending,
    NotSent,
    Applied {
        replayed: bool,
        receipt: OperatorReceiptSummaryV1,
    },
    Recovered(OperatorReceiptSummaryV1),
    NotAvailable,
    Refused(String),
    InvalidReply,
    Unavailable,
    ReceiptPending(OperatorActionIdV1),
    OutcomeUnknown(OperatorActionIdV1),
}

struct Child {
    proposal: ReconciliationProposalV1,
    state: ChildState,
}

enum Request {
    Inspect(Vec<(usize, ReconciliationProposalV1)>),
    Confirm(Vec<(usize, ReconciliationProposalV1)>),
    Recover {
        index: usize,
        proposal: ReconciliationProposalV1,
        action: OperatorActionIdV1,
    },
}

enum Inspection {
    Ready(ReconciliationConfirmationV1),
    NotAvailable,
    Refused(String),
    InvalidReply,
    Unavailable,
}

enum Confirmation {
    Applied {
        replayed: bool,
        receipt: OperatorReceiptSummaryV1,
    },
    NotAvailable,
    Refused(String),
    InvalidReply,
    Unavailable,
    ReceiptPending(OperatorActionIdV1),
    OutcomeUnknown(OperatorActionIdV1),
}

enum Recovery {
    Recovered(OperatorReceiptSummaryV1),
    NotFound,
    Refused(String),
    InvalidReply,
    Unavailable,
}

enum Reply {
    Inspected(Vec<(usize, Inspection)>),
    Confirmed(Vec<(usize, Confirmation)>),
    Recovered { index: usize, result: Recovery },
}

/// One parent interaction over N distinct owner-guarded commands. There is no
/// aggregate success claim: every child retains its own disposition or exact
/// durable receipt, and an unknown child stops later sends.
pub struct ReconciliationBatchComponent {
    requests: SyncSender<Request>,
    replies: Receiver<Reply>,
    children: Vec<Child>,
    pending: bool,
    armed: bool,
    selected: usize,
    notice: String,
    timed_draft: Option<TimedDraft>,
}

impl ReconciliationBatchComponent {
    pub fn new_with_connector<P, F, E>(
        connector: F,
        proposals: Vec<ReconciliationProposalV1>,
    ) -> Result<Self, &'static str>
    where
        P: SagaServicePort + Send + 'static,
        P::Error: Send + 'static,
        F: Fn() -> Result<P, E> + Send + Sync + 'static,
        E: Send + 'static,
    {
        if proposals.is_empty() || proposals.len() > MAX_RECONCILIATION_BATCH {
            return Err("reconciliation batch must contain 1–16 sagas");
        }
        if proposals
            .iter()
            .enumerate()
            .any(|(index, proposal)| proposals[..index].contains(proposal))
        {
            return Err("reconciliation batch contains duplicate proposals");
        }
        let (request_tx, request_rx) = sync_channel(1);
        let (reply_tx, reply_rx) = sync_channel(1);
        std::thread::spawn(move || worker_loop(connector, request_rx, reply_tx));
        let mut value = Self {
            requests: request_tx,
            replies: reply_rx,
            children: proposals
                .into_iter()
                .map(|proposal| Child {
                    proposal,
                    state: ChildState::Inspecting,
                })
                .collect(),
            pending: false,
            armed: false,
            selected: 0,
            notice: "Batch reconciliation inspection queued".into(),
            timed_draft: None,
        };
        value.inspect_unsettled();
        Ok(value)
    }

    pub fn new_timed_with_connector<P, F, E>(
        connector: F,
        sagas: Vec<SagaViewV1>,
        kind: TimedReconciliationKind,
    ) -> Result<Self, &'static str>
    where
        P: SagaServicePort + Send + 'static,
        P::Error: Send + 'static,
        F: Fn() -> Result<P, E> + Send + Sync + 'static,
        E: Send + 'static,
    {
        if sagas.is_empty() || sagas.len() > MAX_RECONCILIATION_BATCH {
            return Err("reconciliation batch must contain 1–16 sagas");
        }
        if sagas
            .iter()
            .enumerate()
            .any(|(index, saga)| sagas[..index].iter().any(|other| other.saga == saga.saga))
        {
            return Err("reconciliation batch contains duplicate sagas");
        }
        if matches!(
            kind,
            TimedReconciliationKind::AdmitFailed
                | TimedReconciliationKind::AdmitSucceeded
                | TimedReconciliationKind::Retry
        ) && sagas.iter().any(|saga| saga.current_effect.is_none())
        {
            return Err("every selected saga must expose an exact current effect");
        }
        let (request_tx, request_rx) = sync_channel(1);
        let (reply_tx, reply_rx) = sync_channel(1);
        std::thread::spawn(move || worker_loop(connector, request_rx, reply_tx));
        Ok(Self {
            requests: request_tx,
            replies: reply_rx,
            children: Vec::new(),
            pending: false,
            armed: false,
            selected: 0,
            notice: "Enter Moira logical time, then press Enter to prepare exact proposals".into(),
            timed_draft: Some(TimedDraft {
                kind,
                sagas,
                logical_time: String::new(),
            }),
        })
    }

    pub const fn editing_logical_time(&self) -> bool {
        self.timed_draft.is_some()
    }

    pub fn poll(&mut self) {
        loop {
            match self.replies.try_recv() {
                Ok(reply) => self.admit(reply),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pending = false;
                    self.notice =
                        "Moira batch worker unavailable; unsettled children were not inferred"
                            .into();
                    break;
                }
            }
        }
    }

    pub fn update(&mut self, input: ReconciliationBatchInput, focused: bool) -> Lifecycle {
        if !focused {
            return Lifecycle::Running;
        }
        if let Some(draft) = &mut self.timed_draft {
            match input {
                ReconciliationBatchInput::Close => return Lifecycle::CloseRequested,
                ReconciliationBatchInput::Digit(value)
                    if value.is_ascii_digit() && draft.logical_time.len() < 20 =>
                {
                    draft.logical_time.push(value);
                }
                ReconciliationBatchInput::Backspace => {
                    draft.logical_time.pop();
                }
                ReconciliationBatchInput::Prepare => self.prepare_timed_proposals(),
                _ => {}
            }
            return Lifecycle::Running;
        }
        match input {
            ReconciliationBatchInput::Close if self.pending || self.recovery_required() => {
                self.notice =
                    "Batch has pending or unknown children; settle them before closing".into();
            }
            ReconciliationBatchInput::Close => return Lifecycle::CloseRequested,
            ReconciliationBatchInput::Back => {
                self.armed = false;
                self.notice = "Batch reconciliation disarmed".into();
            }
            ReconciliationBatchInput::Refresh if !self.pending => {
                self.armed = false;
                if let Some((index, action)) = self.recoverable_child() {
                    let proposal = self.children[index].proposal;
                    self.offer(Request::Recover {
                        index,
                        proposal,
                        action,
                    });
                } else {
                    self.inspect_unsettled();
                }
            }
            ReconciliationBatchInput::Arm
                if !self.pending && !self.recovery_required() && self.candidate_count() > 0 =>
            {
                self.armed = true;
                self.notice = format!(
                    "Batch armed for {} independently receipted saga(s); Enter confirms",
                    self.candidate_count()
                );
            }
            ReconciliationBatchInput::Confirm if self.armed && !self.pending => {
                self.armed = false;
                let mut confirmations = Vec::new();
                for (index, child) in self.children.iter_mut().enumerate() {
                    match child.state {
                        ChildState::Ready(_) | ChildState::NotSent => {}
                        _ => continue,
                    }
                    child.state = ChildState::Pending;
                    confirmations.push((index, child.proposal));
                }
                self.offer(Request::Confirm(confirmations));
            }
            ReconciliationBatchInput::Refresh
            | ReconciliationBatchInput::Arm
            | ReconciliationBatchInput::Confirm
            | ReconciliationBatchInput::Digit(_)
            | ReconciliationBatchInput::Backspace
            | ReconciliationBatchInput::Prepare => {}
        }
        Lifecycle::Running
    }

    fn prepare_timed_proposals(&mut self) {
        let Some(draft) = &self.timed_draft else {
            return;
        };
        let Ok(now) = draft.logical_time.parse::<u64>() else {
            self.notice = "Moira logical time must be an unsigned 64-bit integer".into();
            return;
        };
        let draft = self.timed_draft.take().expect("checked above");
        self.children = draft
            .sagas
            .into_iter()
            .map(|saga| Child {
                proposal: ReconciliationProposalV1 {
                    saga: saga.saga,
                    disposition: match draft.kind {
                        TimedReconciliationKind::Cancel => OperatorDispositionV1::Cancel { now },
                        TimedReconciliationKind::Compensate => {
                            OperatorDispositionV1::Compensate { now }
                        }
                        TimedReconciliationKind::AdmitFailed => {
                            OperatorDispositionV1::AdmitEvidence {
                                effect: saga.current_effect.expect("constructor validated effect"),
                                outcome: SagaEffectOutcomeV1::Failed,
                                observed_at: now,
                            }
                        }
                        TimedReconciliationKind::AdmitSucceeded => {
                            OperatorDispositionV1::AdmitEvidence {
                                effect: saga.current_effect.expect("constructor validated effect"),
                                outcome: SagaEffectOutcomeV1::Succeeded,
                                observed_at: now,
                            }
                        }
                        TimedReconciliationKind::Retry => OperatorDispositionV1::Retry {
                            failed_effect: saga
                                .current_effect
                                .expect("constructor validated effect"),
                            now,
                        },
                    },
                },
                state: ChildState::Inspecting,
            })
            .collect();
        self.notice = "Exact time-bearing proposals prepared; owner inspection queued".into();
        self.inspect_unsettled();
    }

    pub fn move_selection(&mut self, down: bool) {
        if down {
            self.selected = (self.selected + 1).min(self.children.len().saturating_sub(1));
        } else {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    pub const fn request_pending(&self) -> bool {
        self.pending
    }

    pub fn recovery_required(&self) -> bool {
        self.recoverable_child().is_some()
    }

    pub fn disposition(&self) -> ReconciliationBatchDisposition {
        if self.timed_draft.is_some() {
            ReconciliationBatchDisposition::Inspecting
        } else if self.unknown_child().is_some() {
            ReconciliationBatchDisposition::OutcomeUnknown
        } else if self
            .children
            .iter()
            .any(|child| matches!(child.state, ChildState::ReceiptPending(_)))
        {
            ReconciliationBatchDisposition::ReceiptPending
        } else if self.pending {
            ReconciliationBatchDisposition::Pending
        } else if self.armed {
            ReconciliationBatchDisposition::Armed
        } else if self.children.iter().all(|child| {
            matches!(
                child.state,
                ChildState::Applied { .. } | ChildState::Recovered(_)
            )
        }) {
            ReconciliationBatchDisposition::Applied
        } else if self.candidate_count() > 0
            && self.children.iter().all(|child| {
                matches!(
                    child.state,
                    ChildState::Ready(_)
                        | ChildState::NotSent
                        | ChildState::Applied { .. }
                        | ChildState::Recovered(_)
                )
            })
        {
            ReconciliationBatchDisposition::Ready
        } else if self
            .children
            .iter()
            .any(|child| matches!(child.state, ChildState::Inspecting | ChildState::Pending))
        {
            ReconciliationBatchDisposition::Inspecting
        } else {
            ReconciliationBatchDisposition::Partial
        }
    }

    pub fn notice(&self) -> &str {
        &self.notice
    }

    pub fn draw_in(&self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(area);
        let title = if focused {
            " Moira batch reconciliation • focused "
        } else {
            " Moira batch reconciliation "
        };
        if let Some(draft) = &self.timed_draft {
            let mut lines = vec![
                Line::from(format!("workflow: {:?}", draft.kind)),
                Line::from(format!("selected sagas: {}", draft.sagas.len())),
                Line::from(format!("logical time: {}", draft.logical_time)),
                Line::from("This is Moira domain time, not inferred wall-clock time."),
            ];
            lines.extend(draft.sagas.iter().map(|saga| {
                Line::from(format!(
                    "saga {} · current effect {:?} · deadline {}",
                    saga.saga.0, saga.current_effect, saga.deadline
                ))
            }));
            frame.render_widget(
                Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
                body,
            );
            frame.render_widget(
                Paragraph::new(format!(
                    "{} • digits edit time • Backspace • Enter prepare • Esc close",
                    self.notice
                )),
                footer,
            );
            return;
        }
        let mut lines = vec![Line::from(format!(
            "{} sagas • {} ready/not-sent • {} durable • disposition {:?}",
            self.children.len(),
            self.candidate_count(),
            self.children
                .iter()
                .filter(|child| matches!(
                    child.state,
                    ChildState::Applied { .. } | ChildState::Recovered(_)
                ))
                .count(),
            self.disposition()
        ))];
        lines.extend(self.children.iter().enumerate().map(|(index, child)| {
            let marker = if index == self.selected { ">" } else { " " };
            Line::from(format!(
                "{marker} saga {} — {:?} · {}",
                child.proposal.saga.0,
                child.proposal.disposition,
                child_status(&child.state)
            ))
        }));
        lines.push(Line::from(if self.unknown_child().is_some() {
            "OUTCOME UNKNOWN: r inspects that exact action receipt; later children remain not sent"
        } else if self.recovery_required() {
            "APPLIED, RECEIPT PENDING: r retrieves that exact durable receipt before continuing"
        } else if self.armed {
            "ARMED: Enter submits each exact child once; Esc disarms"
        } else if self.candidate_count() > 0 {
            "c arms ready/not-sent children • r refreshes challenges • ↑/↓ inspect"
        } else {
            "No child is currently eligible for submission • r re-inspects unsettled children"
        }));
        frame.render_widget(
            Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
            body,
        );
        frame.render_widget(Paragraph::new(self.notice.as_str()), footer);
    }

    fn candidate_count(&self) -> usize {
        self.children
            .iter()
            .filter(|child| matches!(child.state, ChildState::Ready(_) | ChildState::NotSent))
            .count()
    }

    fn recoverable_child(&self) -> Option<(usize, OperatorActionIdV1)> {
        self.children
            .iter()
            .enumerate()
            .find_map(|(index, child)| match child.state {
                ChildState::OutcomeUnknown(action) | ChildState::ReceiptPending(action) => {
                    Some((index, action))
                }
                _ => None,
            })
    }

    fn unknown_child(&self) -> Option<(usize, OperatorActionIdV1)> {
        self.children.iter().enumerate().find_map(|(index, child)| {
            if let ChildState::OutcomeUnknown(action) = child.state {
                Some((index, action))
            } else {
                None
            }
        })
    }

    fn inspect_unsettled(&mut self) {
        let targets: Vec<_> = self
            .children
            .iter_mut()
            .enumerate()
            .filter_map(|(index, child)| {
                if matches!(
                    child.state,
                    ChildState::Applied { .. } | ChildState::Recovered(_)
                ) {
                    None
                } else {
                    child.state = ChildState::Inspecting;
                    Some((index, child.proposal))
                }
            })
            .collect();
        if targets.is_empty() {
            self.notice = "Every child already has a durable terminal disposition".into();
        } else {
            self.offer(Request::Inspect(targets));
        }
    }

    fn offer(&mut self, request: Request) {
        if self.pending {
            self.notice = "Batch reconciliation request already pending".into();
        } else if self.requests.try_send(request).is_ok() {
            self.pending = true;
            self.notice = "Batch reconciliation request pending".into();
        } else {
            self.notice = "Moira batch worker unavailable".into();
        }
    }

    fn admit(&mut self, reply: Reply) {
        self.pending = false;
        match reply {
            Reply::Inspected(results) => {
                for (index, result) in results {
                    self.children[index].state = match result {
                        Inspection::Ready(value) => ChildState::Ready(value),
                        Inspection::NotAvailable => ChildState::NotAvailable,
                        Inspection::Refused(value) => ChildState::Refused(value),
                        Inspection::InvalidReply => ChildState::InvalidReply,
                        Inspection::Unavailable => ChildState::Unavailable,
                    };
                }
                self.notice = format!(
                    "Inspection settled: {} of {} saga(s) have exact current challenges",
                    self.candidate_count(),
                    self.children.len()
                );
            }
            Reply::Confirmed(results) => {
                for child in &mut self.children {
                    if matches!(child.state, ChildState::Pending) {
                        child.state = ChildState::NotSent;
                    }
                }
                for (index, result) in results {
                    self.children[index].state = match result {
                        Confirmation::Applied { replayed, receipt } => {
                            ChildState::Applied { replayed, receipt }
                        }
                        Confirmation::NotAvailable => ChildState::NotAvailable,
                        Confirmation::Refused(value) => ChildState::Refused(value),
                        Confirmation::InvalidReply => ChildState::InvalidReply,
                        Confirmation::Unavailable => ChildState::Unavailable,
                        Confirmation::ReceiptPending(action) => ChildState::ReceiptPending(action),
                        Confirmation::OutcomeUnknown(action) => ChildState::OutcomeUnknown(action),
                    };
                }
                self.notice = match self.disposition() {
                    ReconciliationBatchDisposition::Applied => {
                        "Every reconciliation child durably applied".into()
                    }
                    ReconciliationBatchDisposition::OutcomeUnknown => {
                        "Batch paused at an unknown child; later children were not sent".into()
                    }
                    _ => "Batch settled with independent child dispositions".into(),
                };
            }
            Reply::Recovered { index, result } => {
                self.children[index].state = match result {
                    Recovery::Recovered(receipt) => ChildState::Recovered(receipt),
                    Recovery::NotFound => {
                        self.notice =
                            "Exact durable receipt is not yet available; batch remains fenced"
                                .into();
                        return;
                    }
                    Recovery::Refused(value) => ChildState::Refused(value),
                    Recovery::InvalidReply => ChildState::InvalidReply,
                    Recovery::Unavailable => {
                        self.notice = "Receipt recovery unavailable; batch remains fenced".into();
                        return;
                    }
                };
                self.notice =
                    "Unknown child recovered from its exact durable operator receipt".into();
            }
        }
    }
}

fn child_status(state: &ChildState) -> String {
    match state {
        ChildState::Inspecting => "inspecting".into(),
        ChildState::Ready(value) => format!("ready · action {}", short_hex(&value.action.0)),
        ChildState::Pending => "pending · current challenge will be acquired just-in-time".into(),
        ChildState::NotSent => "not sent · re-inspection required before submission".into(),
        ChildState::Applied { replayed, receipt } => format!(
            "applied · replayed={replayed} · receipt {} authority {}/{}",
            short_hex(&receipt.action.0),
            receipt.authority_decision,
            receipt.authority_epoch
        ),
        ChildState::Recovered(receipt) => format!(
            "recovered · action {} · authority {}/{}",
            short_hex(&receipt.action.0),
            receipt.authority_decision,
            receipt.authority_epoch
        ),
        ChildState::NotAvailable => "no current challenge".into(),
        ChildState::Refused(value) => format!("refused · {value}"),
        ChildState::InvalidReply => "invalid reply".into(),
        ChildState::Unavailable => "unavailable/not sent".into(),
        ChildState::ReceiptPending(action) => format!(
            "applied; durable receipt read pending · action {}",
            short_hex(&action.0)
        ),
        ChildState::OutcomeUnknown(action) => {
            format!("OUTCOME UNKNOWN · action {}", short_hex(&action.0))
        }
    }
}

fn worker_loop<P, F, E>(connector: F, requests: Receiver<Request>, replies: SyncSender<Reply>)
where
    P: SagaServicePort,
    F: Fn() -> Result<P, E>,
{
    while let Ok(request) = requests.recv() {
        let reply = match request {
            Request::Inspect(targets) => Reply::Inspected(
                targets
                    .into_iter()
                    .map(|(index, proposal)| {
                        let result = match connector() {
                            Ok(port) => {
                                let mut client = SagaReadOnlyClient::new(port);
                                match stable_snapshot(&mut client, proposal, None) {
                                    Observation::Current(snapshot) => {
                                        Inspection::Ready(snapshot.confirmation)
                                    }
                                    Observation::NotAvailable => Inspection::NotAvailable,
                                    Observation::Refused(value) => Inspection::Refused(value),
                                    Observation::InvalidReply => Inspection::InvalidReply,
                                    Observation::ReadUnavailable => Inspection::Unavailable,
                                    _ => Inspection::InvalidReply,
                                }
                            }
                            Err(_) => Inspection::Unavailable,
                        };
                        (index, result)
                    })
                    .collect(),
            ),
            Request::Confirm(confirmations) => {
                let mut results = Vec::new();
                for (index, proposal) in confirmations {
                    let result = match connector() {
                        Ok(port) => {
                            let mut reader = SagaReadOnlyClient::new(port);
                            match stable_snapshot(&mut reader, proposal, None) {
                                Observation::Current(snapshot) => {
                                    let confirmation = snapshot.confirmation;
                                    let action = confirmation.action;
                                    let mut client =
                                        SagaConfirmedReconciliationClient::new(reader.into_port());
                                    match client.confirm(ConfirmedReconciliationV1 {
                                        storage_revision: confirmation.storage_revision,
                                        saga: confirmation.saga,
                                        disposition: confirmation.disposition,
                                        action,
                                        confirmation_digest: confirmation.confirmation_digest,
                                    }) {
                                        Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Applied {
                                            replayed,
                                            ..
                                        })) => {
                                            let mut reader =
                                                SagaReadOnlyClient::new(client.into_port());
                                            match reader.operator_receipt(action) {
                                                Ok(SagaReplyV1::Accepted(
                                                    SagaOutcomeV1::OperatorReceipt(Some(receipt)),
                                                )) if receipt.action == action
                                                    && receipt.saga == proposal.saga =>
                                                {
                                                    Confirmation::Applied { replayed, receipt }
                                                }
                                                _ => Confirmation::ReceiptPending(action),
                                            }
                                        }
                                        Ok(SagaReplyV1::Refused(value)) => {
                                            Confirmation::Refused(format!("{value:?}"))
                                        }
                                        Ok(_) => Confirmation::InvalidReply,
                                        Err(_) => Confirmation::OutcomeUnknown(action),
                                    }
                                }
                                Observation::NotAvailable => Confirmation::NotAvailable,
                                Observation::Refused(value) => Confirmation::Refused(value),
                                Observation::InvalidReply => Confirmation::InvalidReply,
                                Observation::ReadUnavailable => Confirmation::Unavailable,
                                _ => Confirmation::InvalidReply,
                            }
                        }
                        Err(_) => Confirmation::Unavailable,
                    };
                    let stop = matches!(
                        result,
                        Confirmation::OutcomeUnknown(_)
                            | Confirmation::Unavailable
                            | Confirmation::ReceiptPending(_)
                    );
                    results.push((index, result));
                    if stop {
                        break;
                    }
                }
                Reply::Confirmed(results)
            }
            Request::Recover {
                index,
                proposal,
                action,
            } => {
                let result = match connector() {
                    Ok(port) => {
                        let mut client = SagaReadOnlyClient::new(port);
                        match client.operator_receipt(action) {
                            Ok(SagaReplyV1::Accepted(SagaOutcomeV1::OperatorReceipt(Some(
                                receipt,
                            )))) if receipt.action == action && receipt.saga == proposal.saga => {
                                Recovery::Recovered(receipt)
                            }
                            Ok(SagaReplyV1::Accepted(SagaOutcomeV1::OperatorReceipt(None))) => {
                                Recovery::NotFound
                            }
                            Ok(SagaReplyV1::Refused(value)) => {
                                Recovery::Refused(format!("{value:?}"))
                            }
                            Ok(_) => Recovery::InvalidReply,
                            Err(_) => Recovery::Unavailable,
                        }
                    }
                    Err(_) => Recovery::Unavailable,
                };
                Reply::Recovered { index, result }
            }
        };
        if replies.send(reply).is_err() {
            break;
        }
    }
}

fn short_hex(bytes: &[u8]) -> String {
    bytes[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

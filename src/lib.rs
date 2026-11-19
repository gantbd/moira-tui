//! Composable Ratatui presentation and closed governed controls over Moira's
//! capability-shaped public clients.
#![forbid(unsafe_code)]

mod agent;
mod cancel_control;
mod deliberation;
mod open_control;
mod participation;
mod permission_control;
mod prompt_control;
mod reattach_control;
mod reconciliation_batch;
mod reconciliation_control;
pub use agent::AgentComponent;
pub use cancel_control::{CancelControlInput, CancellationControlComponent};
pub use deliberation::{
    DeliberationComponent, DeliberationObservation, HumanGateAgendaObservation,
    deliberation_view_json, observe_deliberation, review_view_json,
};
pub use open_control::{OpenControlComponent, OpenControlInput};
pub use participation::ParticipationComponent;
pub use permission_control::{
    PermissionControlComponent, PermissionControlInput, permission_options_digest,
    validated_permission_request,
};
pub use prompt_control::{
    MAX_INTERACTIVE_PROMPT_BYTES, PromptControlComponent, PromptControlError, PromptControlInput,
};
pub use reattach_control::{ReattachControlComponent, ReattachControlInput};
pub use reconciliation_batch::{
    MAX_RECONCILIATION_BATCH, ReconciliationBatchComponent, ReconciliationBatchDisposition,
    ReconciliationBatchInput, TimedReconciliationKind,
};
pub use reconciliation_control::{ReconciliationControlComponent, ReconciliationControlInput};

use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use moira_client::{SagaReadOnlyClient, SagaServicePort};
use moira_protocol::{
    EffectEvidenceV1, OperatorReceiptSummaryV1, SagaCensusV1, SagaIdV1, SagaOutcomeV1, SagaPageV1,
    SagaReconciliationViewV1, SagaReplyV1, SagaViewV1,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

const PAGE_LIMIT: u64 = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityManifest {
    pub schema: u8,
    pub peer: &'static str,
    pub interactive: &'static [&'static str],
    pub headless: &'static [&'static str],
}

pub const CAPABILITIES: CapabilityManifest = CapabilityManifest {
    schema: 1,
    peer: "moira",
    interactive: &[
        "sagas.inspect.v1",
        "sagas.reconciliation.v1",
        "sagas.evidence.v1",
        "sagas.operator-receipts.v1",
        "sagas.reconciliation.confirm.v1",
        "sagas.reconciliation.recovery.v1",
        "agent.participation.inspect.v1",
        "agent.open.prepare.v1",
        "agent.session.observe.v1",
        "agent.cancel.confirm.v1",
        "agent.reattach.confirm.v1",
        "agent.permission.confirm.v1",
        "agent.prompt.submit.v1",
        "deliberation.inspect.v1",
    ],
    headless: &[
        "sagas.inspect.v1",
        "sagas.reconciliation.confirmation.v1",
        "sagas.reconciliation.confirm.v1",
        "sagas.reconciliation.recovery.v1",
        "agent.session.inspect.v1",
        "agent.events.observe.v1",
        "agent.participation.inspect.v1",
        "agent.prompt.submit.v1",
        "agent.permission.confirmation-view.v1",
        "agent.permission.confirm.v1",
        "agent.cancel.confirmation-view.v1",
        "agent.cancel.confirm.v1",
        "agent.reattach.confirmation-view.v1",
        "agent.reattach.confirm.v1",
        "deliberation.inspect.v1",
    ],
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Input {
    Up,
    Down,
    Toggle,
    Open,
    Back,
    Refresh,
    Close,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lifecycle {
    Running,
    CloseRequested,
}

pub fn translate_key(key: KeyEvent) -> Option<Input> {
    if key.kind != KeyEventKind::Press || key.modifiers != KeyModifiers::NONE {
        return None;
    }
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => Some(Input::Up),
        KeyCode::Down | KeyCode::Char('j') => Some(Input::Down),
        KeyCode::Char(' ') => Some(Input::Toggle),
        KeyCode::Enter => Some(Input::Open),
        KeyCode::Esc | KeyCode::Backspace => Some(Input::Back),
        KeyCode::Char('r') => Some(Input::Refresh),
        KeyCode::Char('q') => Some(Input::Close),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Request {
    Inspect,
    Detail(SagaIdV1),
}

struct WorkerReply {
    request: Request,
    reply: Result<WorkerObservation, WorkerFailure>,
}

enum WorkerFailure {
    Refused(String),
    InvalidReply,
    Transport,
}

enum WorkerObservation {
    Inspect {
        page: SagaPageV1,
        census: SagaCensusV1,
    },
    Detail(DetailObservation),
}

struct DetailObservation {
    reconciliation: Option<SagaReconciliationViewV1>,
    evidence: Vec<EffectEvidenceV1>,
    evidence_truncated: bool,
    receipts: Vec<OperatorReceiptSummaryV1>,
    receipts_truncated: bool,
}

pub struct Component {
    requests: SyncSender<Request>,
    replies: Receiver<WorkerReply>,
    pending: Option<Request>,
    sagas: Vec<SagaViewV1>,
    selected: usize,
    selected_batch: Vec<SagaIdV1>,
    census: Option<SagaCensusV1>,
    detail: Option<DetailObservation>,
    truncated: bool,
    notice: String,
}

impl Component {
    pub fn new<P>(port: P) -> Self
    where
        P: SagaServicePort + Send + 'static,
        P::Error: Send + 'static,
    {
        let (request_tx, request_rx) = sync_channel::<Request>(1);
        let (reply_tx, reply_rx) = sync_channel::<WorkerReply>(1);
        std::thread::spawn(move || {
            let mut client = SagaReadOnlyClient::new(port);
            while let Ok(request) = request_rx.recv() {
                let reply = match request {
                    Request::Inspect => inspect(&mut client),
                    Request::Detail(saga) => detail(&mut client, saga),
                };
                if reply_tx.send(WorkerReply { request, reply }).is_err() {
                    break;
                }
            }
        });
        Self::with_channels(request_tx, reply_rx)
    }

    /// Builds a component that acquires and releases one public-client
    /// connection per observation. This is intended for private daemons that
    /// deliberately serve one connection at a time, so an idle observation
    /// surface cannot head-of-line block a separate guarded control client.
    pub fn new_with_connector<P, F, E>(connector: F) -> Self
    where
        P: SagaServicePort + Send + 'static,
        P::Error: Send + 'static,
        F: Fn() -> Result<P, E> + Send + 'static,
        E: Send + 'static,
    {
        let (request_tx, request_rx) = sync_channel::<Request>(1);
        let (reply_tx, reply_rx) = sync_channel::<WorkerReply>(1);
        std::thread::spawn(move || {
            while let Ok(request) = request_rx.recv() {
                let reply = match connector() {
                    Ok(port) => {
                        let mut client = SagaReadOnlyClient::new(port);
                        match request {
                            Request::Inspect => inspect(&mut client),
                            Request::Detail(saga) => detail(&mut client, saga),
                        }
                    }
                    Err(_) => Err(WorkerFailure::Transport),
                };
                if reply_tx.send(WorkerReply { request, reply }).is_err() {
                    break;
                }
            }
        });
        Self::with_channels(request_tx, reply_rx)
    }

    fn with_channels(request_tx: SyncSender<Request>, reply_rx: Receiver<WorkerReply>) -> Self {
        let mut component = Self {
            requests: request_tx,
            replies: reply_rx,
            pending: None,
            sagas: Vec::new(),
            selected: 0,
            selected_batch: Vec::new(),
            census: None,
            detail: None,
            truncated: false,
            notice: "Moira ready; refresh queued".into(),
        };
        component.offer(Request::Inspect);
        component
    }

    pub fn poll(&mut self) {
        let _ = self.poll_settlement();
    }

    /// Reports settlement of the latest public read for host freshness and
    /// latency instrumentation without exposing Moira authority.
    pub fn poll_settlement(&mut self) -> Option<Result<(), ()>> {
        let mut settlement = None;
        loop {
            match self.replies.try_recv() {
                Ok(reply) => {
                    settlement = Some(if self.admit(reply) { Ok(()) } else { Err(()) });
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.pending.is_some() {
                        settlement = Some(Err(()));
                    }
                    self.pending = None;
                    self.notice = "Moira worker unavailable".into();
                    break;
                }
            }
        }
        settlement
    }

    pub const fn request_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Returns the exact opaque saga selected by the current bounded page.
    /// Hosts may use this only to open another Moira-owned public workflow;
    /// it confers no reconciliation authority.
    pub fn selected_saga(&self) -> Option<SagaIdV1> {
        self.sagas.get(self.selected).map(|saga| saga.saga)
    }

    /// Exact bounded sparse selection in current page order. With no explicit
    /// marks, the focused saga remains the one-item default.
    pub fn selected_sagas(&self) -> Vec<SagaIdV1> {
        self.selected_saga_views()
            .into_iter()
            .map(|saga| saga.saga)
            .collect()
    }

    /// Exact public saga projections behind the sparse workset. This carries
    /// only owner-published observation evidence; it confers no reconciliation
    /// authority and lets a host bind effect-bearing proposals without
    /// inventing or parsing effect identities.
    pub fn selected_saga_views(&self) -> Vec<SagaViewV1> {
        if self.selected_batch.is_empty() {
            return self.sagas.get(self.selected).copied().into_iter().collect();
        }
        self.sagas
            .iter()
            .filter(|saga| self.selected_batch.contains(&saga.saga))
            .copied()
            .collect()
    }

    pub fn update(&mut self, input: Input, focused: bool) -> Lifecycle {
        if !focused {
            return Lifecycle::Running;
        }
        match input {
            Input::Close => return Lifecycle::CloseRequested,
            Input::Refresh => {
                let request = self
                    .detail
                    .as_ref()
                    .and_then(|detail| detail.reconciliation)
                    .map(|detail| Request::Detail(detail.saga.saga))
                    .unwrap_or(Request::Inspect);
                self.offer(request);
            }
            Input::Open if self.detail.is_none() => {
                if let Some(saga) = self.sagas.get(self.selected) {
                    self.offer(Request::Detail(saga.saga));
                }
            }
            Input::Back => self.detail = None,
            Input::Up => self.move_selection(false),
            Input::Down => self.move_selection(true),
            Input::Toggle if self.detail.is_none() => self.toggle_selection(),
            Input::Toggle => {}
            Input::Open => {}
        }
        Lifecycle::Running
    }

    pub fn draw_in(&self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        let title = if focused {
            " Moira • focused "
        } else {
            " Moira "
        };
        if let Some(detail) = &self.detail {
            let Some(reconciliation) = detail.reconciliation else {
                frame.render_widget(
                    Paragraph::new("Saga no longer available")
                        .block(Block::default().title(title).borders(Borders::ALL)),
                    body,
                );
                frame.render_widget(Paragraph::new(self.notice.as_str()), footer);
                return;
            };
            let mut lines = vec![
                Line::from(format!("saga: {:?}", reconciliation.saga.saga)),
                Line::from(format!("phase: {:?}", reconciliation.saga.phase)),
                Line::from(format!(
                    "automatic progress: {:?}",
                    reconciliation.automatic_progress
                )),
                Line::from(format!(
                    "confirmed revision: {}",
                    reconciliation.last_confirmed_revision
                )),
                Line::from(format!(
                    "retained evidence: {}",
                    reconciliation.retained_evidence
                )),
                Line::from(format!("fence: {:?}", reconciliation.fence)),
                Line::from(format!(
                    "evidence page: {}{}",
                    detail.evidence.len(),
                    if detail.evidence_truncated { "+" } else { "" }
                )),
            ];
            for evidence in &detail.evidence {
                lines.push(Line::from(format!(
                    "effect {:?}: {:?} at {}",
                    evidence.effect, evidence.outcome, evidence.observed_at
                )));
            }
            lines.push(Line::from(format!(
                "operator receipts: {}{}",
                detail.receipts.len(),
                if detail.receipts_truncated { "+" } else { "" }
            )));
            for receipt in &detail.receipts {
                lines.push(Line::from(format!(
                    "action {:?}: {:?} authority {}/{} phase {:?}",
                    receipt.action,
                    receipt.disposition,
                    receipt.authority_decision,
                    receipt.authority_epoch,
                    receipt.phase
                )));
            }
            frame.render_widget(
                Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
                body,
            );
        } else {
            let mut items = Vec::new();
            if let Some(census) = self.census {
                items.push(ListItem::new(format!(
                    "census total {} runnable {} awaiting {} fenced {} terminal {} retries {}{}",
                    census.total,
                    census.runnable_effects,
                    census.awaiting_decision,
                    census.fenced,
                    census.terminal,
                    census.retries_consumed,
                    if census.saturated { "+" } else { "" }
                )));
            }
            items.extend(
                self.sagas
                    .iter()
                    .enumerate()
                    .map(|(index, saga)| {
                        let focus = if index == self.selected { ">" } else { " " };
                        let marked = if self.selected_batch.contains(&saga.saga) {
                            "[x]"
                        } else {
                            "[ ]"
                        };
                        ListItem::new(format!("{focus}{marked} {:?}  {:?}", saga.saga, saga.phase))
                    })
                    .collect::<Vec<_>>(),
            );
            frame.render_widget(
                List::new(items).block(Block::default().title(title).borders(Borders::ALL)),
                body,
            );
        }
        frame.render_widget(Paragraph::new(self.notice.as_str()), footer);
    }

    fn offer(&mut self, request: Request) {
        if self.pending.is_some() {
            self.notice = "Moira request already pending".into();
        } else if self.requests.try_send(request).is_ok() {
            self.pending = Some(request);
            self.notice = "Moira refresh pending".into();
        } else {
            self.notice = "Moira worker unavailable".into();
        }
    }

    fn admit(&mut self, worker: WorkerReply) -> bool {
        self.pending = None;
        match worker.reply {
            Ok(WorkerObservation::Inspect { page, census })
                if worker.request == Request::Inspect =>
            {
                self.sagas = page.entries;
                self.selected_batch
                    .retain(|selected| self.sagas.iter().any(|saga| saga.saga == *selected));
                self.census = Some(census);
                self.selected = self.selected.min(self.sagas.len().saturating_sub(1));
                self.truncated = page.next_after.is_some();
                self.notice = if self.truncated {
                    "Bounded saga page; more entries available".into()
                } else {
                    format!("{} saga(s)", self.sagas.len())
                };
                true
            }
            Ok(WorkerObservation::Detail(detail)) if detail.reconciliation.is_some() => {
                self.detail = Some(detail);
                self.notice = "Current evidence and reconciliation view".into();
                true
            }
            Ok(WorkerObservation::Detail(_)) => {
                self.notice = "Saga no longer available".into();
                false
            }
            Err(WorkerFailure::Refused(refusal)) => {
                self.notice = format!("Moira refused: {refusal}");
                false
            }
            Err(WorkerFailure::InvalidReply) => {
                self.notice = "Invalid Moira reply".into();
                false
            }
            Err(WorkerFailure::Transport) => {
                self.notice = "Moira service unavailable; outcome not inferred".into();
                false
            }
            Ok(_) => {
                self.notice = "Invalid Moira reply".into();
                false
            }
        }
    }

    fn move_selection(&mut self, down: bool) {
        if self.detail.is_some() || self.sagas.is_empty() {
            return;
        }
        if down {
            self.selected = (self.selected + 1).min(self.sagas.len() - 1);
        } else {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    fn toggle_selection(&mut self) {
        const MAX_BATCH_SELECTION: usize = 16;
        let Some(saga) = self.selected_saga() else {
            return;
        };
        if let Some(index) = self
            .selected_batch
            .iter()
            .position(|selected| *selected == saga)
        {
            self.selected_batch.remove(index);
            self.notice = format!(
                "{} saga(s) selected for batch work",
                self.selected_batch.len()
            );
        } else if self.selected_batch.len() == MAX_BATCH_SELECTION {
            self.notice = format!("Batch selection is limited to {MAX_BATCH_SELECTION} sagas");
        } else {
            self.selected_batch.push(saga);
            self.notice = format!(
                "{} saga(s) selected for batch work",
                self.selected_batch.len()
            );
        }
    }
}

fn inspect<P: SagaServicePort>(
    client: &mut SagaReadOnlyClient<P>,
) -> Result<WorkerObservation, WorkerFailure> {
    let page = match client
        .inspect(None, PAGE_LIMIT)
        .map_err(|_| WorkerFailure::Transport)?
    {
        SagaReplyV1::Accepted(SagaOutcomeV1::Page(page)) => page,
        SagaReplyV1::Refused(refusal) => {
            return Err(WorkerFailure::Refused(format!("{refusal:?}")));
        }
        _ => return Err(WorkerFailure::InvalidReply),
    };
    let census = match client.census().map_err(|_| WorkerFailure::Transport)? {
        SagaReplyV1::Accepted(SagaOutcomeV1::Census(census)) => census,
        SagaReplyV1::Refused(refusal) => {
            return Err(WorkerFailure::Refused(format!("{refusal:?}")));
        }
        _ => return Err(WorkerFailure::InvalidReply),
    };
    Ok(WorkerObservation::Inspect { page, census })
}

fn detail<P: SagaServicePort>(
    client: &mut SagaReadOnlyClient<P>,
    saga: SagaIdV1,
) -> Result<WorkerObservation, WorkerFailure> {
    let reconciliation = match client
        .reconciliation(saga)
        .map_err(|_| WorkerFailure::Transport)?
    {
        SagaReplyV1::Accepted(SagaOutcomeV1::Reconciliation(detail)) => detail,
        SagaReplyV1::Refused(refusal) => {
            return Err(WorkerFailure::Refused(format!("{refusal:?}")));
        }
        _ => return Err(WorkerFailure::InvalidReply),
    };
    if reconciliation.is_none() {
        return Ok(WorkerObservation::Detail(DetailObservation {
            reconciliation,
            evidence: Vec::new(),
            evidence_truncated: false,
            receipts: Vec::new(),
            receipts_truncated: false,
        }));
    }
    let evidence = match client
        .evidence(saga, None, PAGE_LIMIT)
        .map_err(|_| WorkerFailure::Transport)?
    {
        SagaReplyV1::Accepted(SagaOutcomeV1::Evidence(page)) => page,
        SagaReplyV1::Refused(refusal) => {
            return Err(WorkerFailure::Refused(format!("{refusal:?}")));
        }
        _ => return Err(WorkerFailure::InvalidReply),
    };
    let receipts = match client
        .operator_receipts(None, PAGE_LIMIT)
        .map_err(|_| WorkerFailure::Transport)?
    {
        SagaReplyV1::Accepted(SagaOutcomeV1::OperatorReceipts(page)) => page,
        SagaReplyV1::Refused(refusal) => {
            return Err(WorkerFailure::Refused(format!("{refusal:?}")));
        }
        _ => return Err(WorkerFailure::InvalidReply),
    };
    Ok(WorkerObservation::Detail(DetailObservation {
        reconciliation,
        evidence: evidence.entries,
        evidence_truncated: evidence.next_after.is_some(),
        receipts: receipts
            .entries
            .into_iter()
            .filter(|receipt| receipt.saga == saga)
            .collect(),
        receipts_truncated: receipts.next_after.is_some(),
    }))
}

//! Composable Ratatui presentation and closed governed controls over Moira's
//! capability-shaped public clients.
#![forbid(unsafe_code)]

mod agent;
mod cancel_control;
mod deliberation;
mod participation;
mod permission_control;
mod prompt_control;
mod reattach_control;
mod reconciliation_control;
pub use agent::AgentComponent;
pub use cancel_control::{CancelControlInput, CancellationControlComponent};
pub use deliberation::{DeliberationComponent, DeliberationObservation, observe_deliberation};
pub use participation::ParticipationComponent;
pub use permission_control::{
    PermissionControlComponent, PermissionControlInput, permission_options_digest,
    validated_permission_request,
};
pub use prompt_control::{
    MAX_INTERACTIVE_PROMPT_BYTES, PromptControlComponent, PromptControlError, PromptControlInput,
};
pub use reattach_control::{ReattachControlComponent, ReattachControlInput};
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
        let mut component = Self {
            requests: request_tx,
            replies: reply_rx,
            pending: None,
            sagas: Vec::new(),
            selected: 0,
            census: None,
            detail: None,
            truncated: false,
            notice: "Moira ready; refresh queued".into(),
        };
        component.offer(Request::Inspect);
        component
    }

    pub fn poll(&mut self) {
        loop {
            match self.replies.try_recv() {
                Ok(reply) => self.admit(reply),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.notice = "Moira worker unavailable".into();
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
                        let marker = if index == self.selected { ">" } else { " " };
                        ListItem::new(format!("{marker} {:?}  {:?}", saga.saga, saga.phase))
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

    fn admit(&mut self, worker: WorkerReply) {
        self.pending = None;
        match worker.reply {
            Ok(WorkerObservation::Inspect { page, census })
                if worker.request == Request::Inspect =>
            {
                self.sagas = page.entries;
                self.census = Some(census);
                self.selected = self.selected.min(self.sagas.len().saturating_sub(1));
                self.truncated = page.next_after.is_some();
                self.notice = if self.truncated {
                    "Bounded saga page; more entries available".into()
                } else {
                    format!("{} saga(s)", self.sagas.len())
                };
            }
            Ok(WorkerObservation::Detail(detail)) if detail.reconciliation.is_some() => {
                self.detail = Some(detail);
                self.notice = "Current evidence and reconciliation view".into();
            }
            Ok(WorkerObservation::Detail(_)) => {
                self.notice = "Saga no longer available".into();
            }
            Err(WorkerFailure::Refused(refusal)) => {
                self.notice = format!("Moira refused: {refusal}");
            }
            Err(WorkerFailure::InvalidReply) => self.notice = "Invalid Moira reply".into(),
            Err(WorkerFailure::Transport) => {
                self.notice = "Moira service unavailable; outcome not inferred".into();
            }
            Ok(_) => self.notice = "Invalid Moira reply".into(),
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

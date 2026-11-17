use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use moira_client::SagaServicePort;
use moira_protocol::*;
use ratatui::{Terminal, backend::TestBackend};

#[derive(Default)]
struct State {
    revision: u64,
    confirms: Vec<SagaIdV1>,
    receipts: BTreeMap<OperatorActionIdV1, OperatorReceiptSummaryV1>,
    lost_second_reply: bool,
}

#[derive(Clone)]
struct Port(Arc<Mutex<State>>);

impl SagaServicePort for Port {
    type Error = ();

    fn execute(&mut self, command: SagaCommandV1) -> Result<SagaReplyV1, Self::Error> {
        let mut state = self.0.lock().unwrap();
        if state.revision == 0 {
            state.revision = 2;
        }
        match command {
            SagaCommandV1::Status => {
                Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Status(SagaStatusV1 {
                    revision: state.revision,
                    needs_reopen: false,
                })))
            }
            SagaCommandV1::Reconciliation { saga } => Ok(SagaReplyV1::Accepted(
                SagaOutcomeV1::Reconciliation(Some(view(saga))),
            )),
            SagaCommandV1::ReconciliationConfirmation { proposal } => Ok(SagaReplyV1::Accepted(
                SagaOutcomeV1::ReconciliationConfirmation(reconciliation_confirmation(
                    state.revision,
                    view(proposal.saga),
                    proposal,
                )),
            )),
            SagaCommandV1::ConfirmReconciliation(selected) => {
                let expected = reconciliation_confirmation(
                    state.revision,
                    view(selected.saga),
                    proposal(selected.saga),
                )
                .unwrap();
                assert_eq!(selected.storage_revision, expected.storage_revision);
                assert_eq!(selected.action, expected.action);
                assert_eq!(selected.confirmation_digest, expected.confirmation_digest);
                state.confirms.push(selected.saga);
                state.revision += 1;
                let receipt = receipt(selected.action, selected.saga, state.revision);
                state.receipts.insert(selected.action, receipt);
                if selected.saga == SagaIdV1(2) && !state.lost_second_reply {
                    state.lost_second_reply = true;
                    return Err(());
                }
                Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Applied {
                    storage_revision: state.revision,
                    transition: SagaTransitionV1::Reconciled {
                        saga: selected.saga,
                        action: selected.action,
                        phase: SagaPhaseV1::FencedUncertain,
                        prepared: None,
                    },
                    replayed: false,
                }))
            }
            SagaCommandV1::OperatorReceipt { action } => Ok(SagaReplyV1::Accepted(
                SagaOutcomeV1::OperatorReceipt(state.receipts.get(&action).copied()),
            )),
            _ => panic!("unexpected batch request"),
        }
    }
}

#[test]
fn batch_stops_on_unknown_recovers_exact_receipt_and_resumes_not_sent_children() {
    let shared = Arc::new(Mutex::new(State::default()));
    let source = Arc::clone(&shared);
    let mut batch = moira_tui::ReconciliationBatchComponent::new_with_connector(
        move || -> Result<Port, ()> { Ok(Port(Arc::clone(&source))) },
        vec![
            proposal(SagaIdV1(1)),
            proposal(SagaIdV1(2)),
            proposal(SagaIdV1(3)),
        ],
    )
    .unwrap();

    wait_for(&mut batch, moira_tui::ReconciliationBatchDisposition::Ready);
    batch.update(moira_tui::ReconciliationBatchInput::Arm, true);
    batch.update(moira_tui::ReconciliationBatchInput::Confirm, true);
    wait_for(
        &mut batch,
        moira_tui::ReconciliationBatchDisposition::OutcomeUnknown,
    );
    assert_eq!(shared.lock().unwrap().confirms, [SagaIdV1(1), SagaIdV1(2)]);
    assert!(render(&batch).contains("saga 3 — KeepUncertain · not sent"));

    batch.update(moira_tui::ReconciliationBatchInput::Refresh, true);
    wait_for(&mut batch, moira_tui::ReconciliationBatchDisposition::Ready);
    let recovered = render(&batch);
    assert!(recovered.contains("saga 2 — KeepUncertain · recovered"));
    assert!(recovered.contains("authority 4/"));

    batch.update(moira_tui::ReconciliationBatchInput::Arm, true);
    batch.update(moira_tui::ReconciliationBatchInput::Confirm, true);
    wait_for(
        &mut batch,
        moira_tui::ReconciliationBatchDisposition::Applied,
    );
    assert_eq!(
        shared.lock().unwrap().confirms,
        [SagaIdV1(1), SagaIdV1(2), SagaIdV1(3)]
    );
    let complete = render(&batch);
    assert!(complete.contains("3 durable"));
    assert!(complete.contains("Every reconciliation child durably applied"));
}

#[test]
fn batch_renders_the_exact_time_bearing_disposition_before_arming() {
    let shared = Arc::new(Mutex::new(State::default()));
    let source = Arc::clone(&shared);
    let mut batch = moira_tui::ReconciliationBatchComponent::new_timed_with_connector(
        move || -> Result<Port, ()> { Ok(Port(Arc::clone(&source))) },
        vec![view(SagaIdV1(1)).saga],
        moira_tui::TimedReconciliationKind::Cancel,
    )
    .unwrap();

    assert!(batch.editing_logical_time());
    assert!(render(&batch).contains("not inferred wall-clock time"));
    batch.update(moira_tui::ReconciliationBatchInput::Prepare, true);
    assert!(batch.notice().contains("unsigned 64-bit integer"));
    batch.update(moira_tui::ReconciliationBatchInput::Digit('4'), true);
    batch.update(moira_tui::ReconciliationBatchInput::Digit('2'), true);
    batch.update(moira_tui::ReconciliationBatchInput::Prepare, true);
    wait_for(&mut batch, moira_tui::ReconciliationBatchDisposition::Ready);
    assert!(!batch.editing_logical_time());
    assert!(render(&batch).contains("saga 1 — Cancel { now: 42 } · ready"));
}

fn proposal(saga: SagaIdV1) -> ReconciliationProposalV1 {
    ReconciliationProposalV1 {
        saga,
        disposition: OperatorDispositionV1::KeepUncertain,
    }
}

fn view(saga: SagaIdV1) -> SagaReconciliationViewV1 {
    SagaReconciliationViewV1 {
        saga: SagaViewV1 {
            saga,
            phase: SagaPhaseV1::FencedUncertain,
            deadline: 20,
            timer: TimerIdV1(saga.0),
            current_effect: Some(SagaEffectIdV1(saga.0)),
            current_kind: Some(SagaEffectKindV1::Forward),
            retries_used: 0,
            retry_budget: 2,
        },
        last_confirmed_revision: 1,
        retained_evidence: 1,
        fence: Some(FenceCauseV1::EffectUncertain {
            effect: SagaEffectIdV1(saga.0),
            kind: SagaEffectKindV1::Forward,
            observed_at: 11,
        }),
        automatic_progress: AutomaticProgressV1::BlockedUncertain,
    }
}

fn receipt(
    action: OperatorActionIdV1,
    saga: SagaIdV1,
    authority_epoch: u64,
) -> OperatorReceiptSummaryV1 {
    OperatorReceiptSummaryV1 {
        action,
        saga,
        authority_decision: 4,
        authority_epoch,
        disposition: OperatorDispositionV1::KeepUncertain,
        phase: SagaPhaseV1::FencedUncertain,
        prepared: None,
    }
}

fn wait_for(
    batch: &mut moira_tui::ReconciliationBatchComponent,
    expected: moira_tui::ReconciliationBatchDisposition,
) {
    for _ in 0..1000 {
        batch.poll();
        if batch.disposition() == expected {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("batch did not reach {expected:?}: {}", batch.notice());
}

fn render(batch: &moira_tui::ReconciliationBatchComponent) -> String {
    let mut terminal = Terminal::new(TestBackend::new(150, 18)).unwrap();
    terminal
        .draw(|frame| batch.draw_in(frame, frame.area(), true))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

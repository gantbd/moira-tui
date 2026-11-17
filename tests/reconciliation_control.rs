use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{Sender, channel},
    },
    thread,
    time::Duration,
};

use moira_client::SagaServicePort;
use moira_protocol::*;
use ratatui::{Terminal, backend::TestBackend};

enum Mutation {
    Accept(Sender<ConfirmedReconciliationV1>),
    UnknownThenReceipt(Arc<AtomicUsize>),
}

struct Port {
    read_step: usize,
    mutation: Mutation,
}

impl SagaServicePort for Port {
    type Error = ();

    fn execute(&mut self, command: SagaCommandV1) -> Result<SagaReplyV1, Self::Error> {
        let proposal = proposal();
        let confirmation = reconciliation_confirmation(2, view(), proposal).unwrap();
        match command {
            SagaCommandV1::ConfirmReconciliation(selected) => match &self.mutation {
                Mutation::Accept(sent) => {
                    sent.send(selected).unwrap();
                    Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Applied {
                        storage_revision: 2,
                        transition: SagaTransitionV1::Reconciled {
                            saga: proposal.saga,
                            action: confirmation.action,
                            phase: SagaPhaseV1::FencedUncertain,
                            prepared: None,
                        },
                        replayed: false,
                    }))
                }
                Mutation::UnknownThenReceipt(calls) => {
                    calls.fetch_add(1, Ordering::Relaxed);
                    Err(())
                }
            },
            SagaCommandV1::OperatorReceipt { action } => {
                assert_eq!(action, confirmation.action);
                Ok(SagaReplyV1::Accepted(SagaOutcomeV1::OperatorReceipt(Some(
                    receipt(confirmation.action),
                ))))
            }
            command => {
                let expected = match self.read_step % 5 {
                    0 | 3 => SagaCommandV1::Status,
                    1 => SagaCommandV1::Reconciliation {
                        saga: proposal.saga,
                    },
                    2 | 4 => SagaCommandV1::ReconciliationConfirmation { proposal },
                    _ => unreachable!(),
                };
                assert_eq!(command, expected);
                self.read_step += 1;
                Ok(SagaReplyV1::Accepted(match (self.read_step - 1) % 5 {
                    0 | 3 => SagaOutcomeV1::Status(SagaStatusV1 {
                        revision: 2,
                        needs_reopen: false,
                    }),
                    1 => SagaOutcomeV1::Reconciliation(Some(view())),
                    2 | 4 => SagaOutcomeV1::ReconciliationConfirmation(Some(confirmation)),
                    _ => unreachable!(),
                }))
            }
        }
    }
}

#[test]
fn reconciliation_requires_arm_then_exact_server_confirmation() {
    let (sent, received) = channel();
    let mut component = moira_tui::ReconciliationControlComponent::new(
        Port {
            read_step: 0,
            mutation: Mutation::Accept(sent),
        },
        proposal(),
    );
    wait_for(&mut component, "server challenge:");
    component.update(moira_tui::ReconciliationControlInput::Confirm, true);
    assert!(received.try_recv().is_err());
    component.update(moira_tui::ReconciliationControlInput::Arm, true);
    assert!(render(&component).contains("ARMED"));
    component.update(moira_tui::ReconciliationControlInput::Confirm, true);
    let selected = received.recv_timeout(Duration::from_secs(1)).unwrap();
    let expected = reconciliation_confirmation(2, view(), proposal()).unwrap();
    assert_eq!(selected.storage_revision, expected.storage_revision);
    assert_eq!(selected.action, expected.action);
    assert_eq!(selected.confirmation_digest, expected.confirmation_digest);
    wait_for(&mut component, "Reconciliation durably accepted");
}

#[test]
fn unknown_reconciliation_fences_until_exact_receipt_recovery() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut component = moira_tui::ReconciliationControlComponent::new(
        Port {
            read_step: 0,
            mutation: Mutation::UnknownThenReceipt(Arc::clone(&calls)),
        },
        proposal(),
    );
    wait_for(&mut component, "server challenge:");
    component.update(moira_tui::ReconciliationControlInput::Arm, true);
    component.update(moira_tui::ReconciliationControlInput::Confirm, true);
    wait_for(&mut component, "OUTCOME UNKNOWN");
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        component.update(moira_tui::ReconciliationControlInput::Close, true),
        moira_tui::Lifecycle::Running
    );

    component.update(moira_tui::ReconciliationControlInput::Arm, true);
    component.update(moira_tui::ReconciliationControlInput::Confirm, true);
    thread::sleep(Duration::from_millis(5));
    component.poll();
    assert_eq!(calls.load(Ordering::Relaxed), 1);

    component.update(moira_tui::ReconciliationControlInput::Refresh, true);
    wait_for(&mut component, "durable receipt:");
    assert!(render(&component).contains("Unknown outcome recovered"));
}

fn proposal() -> ReconciliationProposalV1 {
    ReconciliationProposalV1 {
        saga: SagaIdV1(0),
        disposition: OperatorDispositionV1::KeepUncertain,
    }
}

fn view() -> SagaReconciliationViewV1 {
    SagaReconciliationViewV1 {
        saga: SagaViewV1 {
            saga: SagaIdV1(0),
            phase: SagaPhaseV1::FencedUncertain,
            deadline: 20,
            timer: TimerIdV1(0),
            current_effect: Some(SagaEffectIdV1(0)),
            current_kind: Some(SagaEffectKindV1::Forward),
            retries_used: 0,
            retry_budget: 2,
        },
        last_confirmed_revision: 1,
        retained_evidence: 1,
        fence: Some(FenceCauseV1::EffectUncertain {
            effect: SagaEffectIdV1(0),
            kind: SagaEffectKindV1::Forward,
            observed_at: 11,
        }),
        automatic_progress: AutomaticProgressV1::BlockedUncertain,
    }
}

fn receipt(action: OperatorActionIdV1) -> OperatorReceiptSummaryV1 {
    OperatorReceiptSummaryV1 {
        action,
        saga: SagaIdV1(0),
        authority_decision: 4,
        authority_epoch: 5,
        disposition: OperatorDispositionV1::KeepUncertain,
        phase: SagaPhaseV1::FencedUncertain,
        prepared: None,
    }
}

fn wait_for(component: &mut moira_tui::ReconciliationControlComponent, needle: &str) {
    for _ in 0..100 {
        component.poll();
        if render(component).contains(needle) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("component did not render {needle}");
}

fn render(component: &moira_tui::ReconciliationControlComponent) -> String {
    let mut terminal = Terminal::new(TestBackend::new(150, 18)).unwrap();
    terminal
        .draw(|frame| component.draw_in(frame, frame.area(), true))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

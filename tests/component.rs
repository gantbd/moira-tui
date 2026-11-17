use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use moira_client::SagaServicePort;
use moira_protocol::{
    AutomaticProgressV1, EffectEvidencePageV1, EffectEvidenceV1, FenceCauseV1, OperatorActionIdV1,
    OperatorDispositionV1, OperatorReceiptPageV1, OperatorReceiptSummaryV1, SagaCensusV1,
    SagaCommandV1, SagaEffectIdV1, SagaEffectKindV1, SagaEffectOutcomeV1, SagaIdV1, SagaOutcomeV1,
    SagaPageV1, SagaPhaseV1, SagaReconciliationViewV1, SagaReplyV1, SagaViewV1, TimerIdV1,
};
use ratatui::{Terminal, backend::TestBackend};

struct Port;

impl SagaServicePort for Port {
    type Error = ();

    fn execute(&mut self, command: SagaCommandV1) -> Result<SagaReplyV1, Self::Error> {
        match command {
            SagaCommandV1::Inspect { limit: 64, .. } => {
                Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Page(SagaPageV1 {
                    entries: vec![SagaViewV1 {
                        saga: SagaIdV1(7),
                        phase: SagaPhaseV1::ForwardPending,
                        deadline: 20,
                        timer: TimerIdV1(9),
                        current_effect: None,
                        current_kind: None,
                        retries_used: 0,
                        retry_budget: 2,
                    }],
                    next_after: None,
                })))
            }
            SagaCommandV1::Census => {
                Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Census(SagaCensusV1 {
                    total: 1,
                    runnable_effects: 1,
                    awaiting_decision: 0,
                    fenced: 0,
                    terminal: 0,
                    retries_consumed: 0,
                    saturated: false,
                })))
            }
            SagaCommandV1::Reconciliation { saga: SagaIdV1(7) } => Ok(SagaReplyV1::Accepted(
                SagaOutcomeV1::Reconciliation(Some(SagaReconciliationViewV1 {
                    saga: detail_saga(),
                    last_confirmed_revision: 2,
                    retained_evidence: 1,
                    fence: Some(FenceCauseV1::EffectUncertain {
                        effect: SagaEffectIdV1(8),
                        kind: SagaEffectKindV1::Forward,
                        observed_at: 11,
                    }),
                    automatic_progress: AutomaticProgressV1::BlockedUncertain,
                })),
            )),
            SagaCommandV1::Evidence {
                saga: SagaIdV1(7),
                limit: 64,
                ..
            } => Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Evidence(
                EffectEvidencePageV1 {
                    entries: vec![EffectEvidenceV1 {
                        effect: SagaEffectIdV1(8),
                        outcome: SagaEffectOutcomeV1::Uncertain,
                        observed_at: 11,
                    }],
                    next_after: None,
                },
            ))),
            SagaCommandV1::OperatorReceipts { limit: 64, .. } => Ok(SagaReplyV1::Accepted(
                SagaOutcomeV1::OperatorReceipts(OperatorReceiptPageV1 {
                    entries: vec![OperatorReceiptSummaryV1 {
                        action: OperatorActionIdV1([4; 16]),
                        saga: SagaIdV1(7),
                        authority_decision: 9,
                        authority_epoch: 10,
                        disposition: OperatorDispositionV1::KeepUncertain,
                        phase: SagaPhaseV1::FencedUncertain,
                        prepared: None,
                    }],
                    next_after: None,
                }),
            )),
            _ => panic!("unexpected component request"),
        }
    }
}

struct ReleasingPort(Arc<AtomicUsize>);

impl Drop for ReleasingPort {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl SagaServicePort for ReleasingPort {
    type Error = ();

    fn execute(&mut self, command: SagaCommandV1) -> Result<SagaReplyV1, Self::Error> {
        Port.execute(command)
    }
}

fn detail_saga() -> SagaViewV1 {
    SagaViewV1 {
        saga: SagaIdV1(7),
        phase: SagaPhaseV1::FencedUncertain,
        deadline: 20,
        timer: TimerIdV1(9),
        current_effect: Some(SagaEffectIdV1(8)),
        current_kind: Some(SagaEffectKindV1::Forward),
        retries_used: 0,
        retry_budget: 2,
    }
}

#[test]
fn component_renders_public_saga_projection() {
    let mut component = moira_tui::Component::new(Port);
    for _ in 0..100 {
        component.poll();
        let mut terminal = Terminal::new(TestBackend::new(100, 10)).unwrap();
        terminal
            .draw(|frame| component.draw_in(frame, frame.area(), true))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        if rendered.contains("ForwardPending") {
            assert!(rendered.contains("SagaIdV1(7)"));
            assert!(rendered.contains("census total 1 runnable 1"));
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("Moira component did not render its accepted saga page");
}

#[test]
fn connector_component_releases_the_private_daemon_after_each_observation() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let worker_dropped = Arc::clone(&dropped);
    let mut component = moira_tui::Component::new_with_connector(move || {
        Ok::<_, ()>(ReleasingPort(Arc::clone(&worker_dropped)))
    });
    for _ in 0..100 {
        if component.poll_settlement().is_some() {
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("connector-backed observation did not settle");
}

#[test]
fn selected_saga_renders_bounded_evidence_and_operator_receipts() {
    let mut component = moira_tui::Component::new(Port);
    for _ in 0..100 {
        component.poll();
        let rendered = render(&component, 120, 18);
        if rendered.contains("ForwardPending") {
            assert_eq!(component.selected_saga(), Some(SagaIdV1(7)));
            component.update(moira_tui::Input::Open, true);
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    for _ in 0..100 {
        component.poll();
        let rendered = render(&component, 120, 18);
        if rendered.contains("evidence page: 1") {
            assert!(rendered.contains("Uncertain at 11"));
            assert!(rendered.contains("operator receipts: 1"));
            assert!(rendered.contains("authority 9/10"));
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("Moira component did not render bounded evidence and receipts");
}

struct ManyPort;

impl SagaServicePort for ManyPort {
    type Error = ();

    fn execute(&mut self, command: SagaCommandV1) -> Result<SagaReplyV1, Self::Error> {
        match command {
            SagaCommandV1::Inspect { limit: 64, .. } => {
                Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Page(SagaPageV1 {
                    entries: (0..20)
                        .map(|id| SagaViewV1 {
                            saga: SagaIdV1(id),
                            phase: SagaPhaseV1::FencedUncertain,
                            deadline: 20,
                            timer: TimerIdV1(id),
                            current_effect: None,
                            current_kind: None,
                            retries_used: 0,
                            retry_budget: 2,
                        })
                        .collect(),
                    next_after: None,
                })))
            }
            SagaCommandV1::Census => {
                Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Census(SagaCensusV1 {
                    total: 20,
                    runnable_effects: 0,
                    awaiting_decision: 0,
                    fenced: 20,
                    terminal: 0,
                    retries_consumed: 0,
                    saturated: false,
                })))
            }
            _ => panic!("unexpected multi-selection request"),
        }
    }
}

#[test]
fn sparse_batch_selection_is_visible_bounded_and_keeps_one_item_default() {
    let mut component = moira_tui::Component::new(ManyPort);
    for _ in 0..100 {
        component.poll();
        if render(&component, 120, 24).contains("SagaIdV1(19)") {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(component.selected_sagas(), vec![SagaIdV1(0)]);
    for index in 0..16 {
        if index > 0 {
            component.update(moira_tui::Input::Down, true);
        }
        component.update(moira_tui::Input::Toggle, true);
    }
    component.update(moira_tui::Input::Down, true);
    component.update(moira_tui::Input::Toggle, true);
    assert_eq!(component.selected_sagas().len(), 16);
    let rendered = render(&component, 120, 24);
    assert!(rendered.contains("[x] SagaIdV1(0)"));
    assert!(rendered.contains("Batch selection is limited to 16 sagas"));
}

fn render(component: &moira_tui::Component, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
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

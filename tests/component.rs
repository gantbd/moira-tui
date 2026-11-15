use std::{thread, time::Duration};

use moira_client::SagaServicePort;
use moira_protocol::{
    SagaCensusV1, SagaCommandV1, SagaIdV1, SagaOutcomeV1, SagaPageV1, SagaPhaseV1, SagaReplyV1,
    SagaViewV1, TimerIdV1,
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
            _ => panic!("unexpected component request"),
        }
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

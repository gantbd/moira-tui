use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use moira_client::AgentParticipationServicePort;
use moira_protocol::{WorkflowIdV1, agent_participation as p, deliberation as d};
use ratatui::{Terminal, backend::TestBackend};

struct Port {
    view: p::ParticipationViewV1,
    apply_calls: Arc<AtomicUsize>,
    fail_apply: bool,
}

impl AgentParticipationServicePort for Port {
    type Error = ();

    fn execute_participation(&mut self, command: p::CommandV1) -> Result<p::ReplyV1, Self::Error> {
        let outcome = match command {
            p::CommandV1::Participation { participation }
                if participation == self.view.binding.participation =>
            {
                p::OutcomeV1::Participation(Some(self.view.clone()))
            }
            p::CommandV1::PrepareOpen { participation }
                if participation == self.view.binding.participation =>
            {
                self.apply_calls.fetch_add(1, Ordering::Relaxed);
                if self.fail_apply {
                    return Err(());
                }
                let expected = p::prepared_open_operation(14, &self.view).unwrap();
                self.view.revision = 1;
                self.view.pending = Some(expected);
                p::OutcomeV1::Applied {
                    storage_revision: 15,
                    participation: self.view.clone(),
                    replayed: false,
                }
            }
            other => panic!("unexpected command: {other:?}"),
        };
        Ok(p::ReplyV1::Accepted(Box::new(outcome)))
    }
}

#[test]
fn open_requires_review_arm_and_owner_derived_confirmation() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut component = moira_tui::OpenControlComponent::new(
        Port {
            view: view(),
            apply_calls: Arc::clone(&calls),
            fail_apply: false,
        },
        p::ParticipationIdV1([1; 16]),
    );
    wait_for(&mut component, "c arms owner-derived open");
    component.update(moira_tui::OpenControlInput::Confirm, true);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    component.update(moira_tui::OpenControlInput::Arm, true);
    assert!(render(&component).contains("ARMED"));
    component.update(moira_tui::OpenControlInput::Confirm, true);
    wait_for(&mut component, "durably prepared");
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    let screen = render(&component);
    assert!(screen.contains("task key: 07070707"));
    assert!(screen.contains("pending operation: Some(Open"));
}

#[test]
fn unknown_open_outcome_fences_another_apply_until_inspection() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut component = moira_tui::OpenControlComponent::new(
        Port {
            view: view(),
            apply_calls: Arc::clone(&calls),
            fail_apply: true,
        },
        p::ParticipationIdV1([1; 16]),
    );
    wait_for(&mut component, "c arms owner-derived open");
    component.update(moira_tui::OpenControlInput::Arm, true);
    component.update(moira_tui::OpenControlInput::Confirm, true);
    wait_for(&mut component, "OUTCOME UNKNOWN");
    component.update(moira_tui::OpenControlInput::Arm, true);
    component.update(moira_tui::OpenControlInput::Confirm, true);
    thread::sleep(Duration::from_millis(5));
    component.poll();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

fn wait_for(component: &mut moira_tui::OpenControlComponent, needle: &str) {
    for _ in 0..200 {
        component.poll();
        if render(component).contains(needle) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("component did not render {needle}");
}

fn render(component: &moira_tui::OpenControlComponent) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 18)).unwrap();
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

fn view() -> p::ParticipationViewV1 {
    p::ParticipationViewV1 {
        binding: p::BindingV1 {
            participation: p::ParticipationIdV1([1; 16]),
            workflow: WorkflowIdV1([2; 16]),
            deliberation: d::DeliberationIdV1([3; 16]),
            member: d::EnsembleMemberIdV1([4; 16]),
            session: p::AgentSessionIdV1([5; 16]),
            profile: p::ProfileIdV1([6; 16]),
            attempt: p::AttemptV1 {
                task_key: [7; 16],
                incarnation: 8,
                attempt: 9,
            },
            method: d::MethodRefV1 {
                machine_digest: [10; 32],
                state_digest: [11; 32],
            },
            grounding: d::GroundingRefV1 {
                authority_digest: [12; 32],
                surface_digest: [13; 32],
                depth: d::SurfaceDepthV1::Reference,
            },
            persona: d::PersonaLensV1::Historian,
        },
        revision: 0,
        phase: p::PhaseV1::Admitted,
        session_revision: None,
        last_event_cursor: None,
        profile_document_digest: None,
        capabilities: p::CapabilitiesV1::default(),
        active_turn: None,
        permission: None,
        terminal: None,
        pending: None,
    }
}

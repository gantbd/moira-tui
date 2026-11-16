use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{Sender, channel},
    },
    thread,
    time::Duration,
};

use moira_client::AgentParticipationServicePort;
use moira_protocol::{WorkflowIdV1, agent_participation as p, deliberation as d};
use ratatui::{Terminal, backend::TestBackend};

enum Mutation {
    Accept(Sender<p::ConfirmedReattachV1>),
    Unknown(Arc<AtomicUsize>),
}

struct Port {
    step: usize,
    mutation: Mutation,
}

impl AgentParticipationServicePort for Port {
    type Error = ();

    fn execute_participation(&mut self, command: p::CommandV1) -> Result<p::ReplyV1, Self::Error> {
        let view = view();
        let challenge = p::reattach_confirmation(24, &view).unwrap();
        if let p::CommandV1::ConfirmReattach(confirmation) = command {
            return match &self.mutation {
                Mutation::Accept(sent) => {
                    sent.send(confirmation).unwrap();
                    let mut prepared = view;
                    prepared.revision += 1;
                    prepared.pending = p::confirmed_reattach_operation(
                        challenge,
                        confirmation,
                        prepared.binding.attempt,
                    );
                    Ok(p::ReplyV1::Accepted(Box::new(p::OutcomeV1::Applied {
                        storage_revision: 25,
                        participation: prepared,
                        replayed: false,
                    })))
                }
                Mutation::Unknown(calls) => {
                    calls.fetch_add(1, Ordering::Relaxed);
                    Err(())
                }
            };
        }
        let expected = if self.step.is_multiple_of(2) {
            p::CommandV1::Participation {
                participation: view.binding.participation,
            }
        } else {
            p::CommandV1::ReattachConfirmation {
                participation: view.binding.participation,
            }
        };
        assert_eq!(command, expected);
        self.step += 1;
        Ok(p::ReplyV1::Accepted(Box::new(
            if !self.step.is_multiple_of(2) {
                p::OutcomeV1::Participation(Some(view))
            } else {
                p::OutcomeV1::ReattachConfirmation(Some(challenge))
            },
        )))
    }
}

#[test]
fn reattach_requires_arm_then_exact_server_challenge_confirmation() {
    let (sent, received) = channel();
    let mut component = moira_tui::ReattachControlComponent::new(
        Port {
            step: 0,
            mutation: Mutation::Accept(sent),
        },
        p::ParticipationIdV1([1; 16]),
    );
    wait_for(&mut component, "server challenge:");
    component.update(moira_tui::ReattachControlInput::Confirm, true);
    assert!(received.try_recv().is_err());
    component.update(moira_tui::ReattachControlInput::Arm, true);
    assert!(render(&component).contains("ARMED"));
    component.update(moira_tui::ReattachControlInput::Confirm, true);
    let confirmation = received.recv_timeout(Duration::from_secs(1)).unwrap();
    let challenge = p::reattach_confirmation(24, &view()).unwrap();
    assert_eq!(confirmation.participation, challenge.participation);
    assert_eq!(
        confirmation.confirmation_digest,
        challenge.confirmation_digest
    );
    wait_for(&mut component, "Reattach durably prepared");
}

#[test]
fn unknown_reattach_outcome_fences_another_confirmation_until_refresh() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut component = moira_tui::ReattachControlComponent::new(
        Port {
            step: 0,
            mutation: Mutation::Unknown(Arc::clone(&calls)),
        },
        p::ParticipationIdV1([1; 16]),
    );
    wait_for(&mut component, "server challenge:");
    component.update(moira_tui::ReattachControlInput::Arm, true);
    component.update(moira_tui::ReattachControlInput::Confirm, true);
    wait_for(&mut component, "OUTCOME UNKNOWN");
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    component.update(moira_tui::ReattachControlInput::Arm, true);
    component.update(moira_tui::ReattachControlInput::Confirm, true);
    thread::sleep(Duration::from_millis(5));
    component.poll();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

fn wait_for(component: &mut moira_tui::ReattachControlComponent, needle: &str) {
    for _ in 0..100 {
        component.poll();
        if render(component).contains(needle) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("component did not render {needle}");
}

fn render(component: &moira_tui::ReattachControlComponent) -> String {
    let mut terminal = Terminal::new(TestBackend::new(110, 16)).unwrap();
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
        revision: 14,
        phase: p::PhaseV1::ReattachRequired,
        session_revision: Some(15),
        last_event_cursor: Some(16),
        profile_document_digest: Some([17; 32]),
        capabilities: p::CapabilitiesV1 {
            resume_session: true,
            ..p::CapabilitiesV1::default()
        },
        active_turn: None,
        permission: None,
        terminal: None,
        pending: None,
    }
}

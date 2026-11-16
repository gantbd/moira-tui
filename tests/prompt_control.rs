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
use sha2::{Digest, Sha256};

enum Mutation {
    Accept(Sender<Vec<u8>>),
    Unknown(Arc<AtomicUsize>),
}

struct Port(Mutation);

impl AgentParticipationServicePort for Port {
    type Error = ();

    fn execute_participation(&mut self, command: p::CommandV1) -> Result<p::ReplyV1, Self::Error> {
        match command {
            p::CommandV1::Participation { participation } => {
                assert_eq!(participation, p::ParticipationIdV1([1; 16]));
                Ok(p::ReplyV1::Accepted(Box::new(p::OutcomeV1::Participation(
                    Some(view()),
                ))))
            }
            p::CommandV1::SubmitPrompt(submission) => {
                assert_eq!(submission.participation, p::ParticipationIdV1([1; 16]));
                match &self.0 {
                    Mutation::Accept(sent) => {
                        sent.send(submission.prompt).unwrap();
                        Ok(p::ReplyV1::Accepted(Box::new(p::OutcomeV1::Applied {
                            storage_revision: 25,
                            participation: view(),
                            replayed: false,
                        })))
                    }
                    Mutation::Unknown(calls) => {
                        calls.fetch_add(1, Ordering::Relaxed);
                        Err(())
                    }
                }
            }
            _ => panic!("unexpected prompt command"),
        }
    }
}

#[test]
fn prompt_requires_stable_ready_state_then_arm_and_exact_byte_confirmation() {
    let prompt = b"exact\0prompt bytes".to_vec();
    let (sent, received) = channel();
    let mut component = moira_tui::PromptControlComponent::try_new(
        Port(Mutation::Accept(sent)),
        p::ParticipationIdV1([1; 16]),
        prompt.clone(),
    )
    .unwrap();
    wait_for(&mut component, "Stable governed participation retained");
    let rendered = render(&component);
    assert!(rendered.contains("prompt preview: exact\\x00prompt bytes"));
    assert!(rendered.contains(&hex(&Sha256::digest(&prompt))));

    component.update(moira_tui::PromptControlInput::Confirm, true);
    assert!(received.try_recv().is_err());
    component.update(moira_tui::PromptControlInput::Arm, true);
    assert!(render(&component).contains("ARMED"));
    component.update(moira_tui::PromptControlInput::Confirm, true);
    assert_eq!(
        received.recv_timeout(Duration::from_secs(1)).unwrap(),
        prompt
    );
    wait_for(&mut component, "Prompt durably accepted");
}

#[test]
fn unknown_prompt_outcome_fences_a_second_submission_until_refresh() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut component = moira_tui::PromptControlComponent::try_new(
        Port(Mutation::Unknown(Arc::clone(&calls))),
        p::ParticipationIdV1([1; 16]),
        b"bounded prompt".to_vec(),
    )
    .unwrap();
    wait_for(&mut component, "Stable governed participation retained");
    component.update(moira_tui::PromptControlInput::Arm, true);
    component.update(moira_tui::PromptControlInput::Confirm, true);
    wait_for(&mut component, "OUTCOME UNKNOWN");
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    component.update(moira_tui::PromptControlInput::Arm, true);
    component.update(moira_tui::PromptControlInput::Confirm, true);
    thread::sleep(Duration::from_millis(5));
    component.poll();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn prompt_component_rejects_empty_and_oversized_content_before_worker_creation() {
    assert!(matches!(
        moira_tui::PromptControlComponent::try_new(
            Port(Mutation::Unknown(Arc::new(AtomicUsize::new(0)))),
            p::ParticipationIdV1([1; 16]),
            Vec::new(),
        ),
        Err(moira_tui::PromptControlError::InvalidPrompt)
    ));
    assert!(matches!(
        moira_tui::PromptControlComponent::try_new(
            Port(Mutation::Unknown(Arc::new(AtomicUsize::new(0)))),
            p::ParticipationIdV1([1; 16]),
            vec![0; moira_tui::MAX_INTERACTIVE_PROMPT_BYTES + 1],
        ),
        Err(moira_tui::PromptControlError::InvalidPrompt)
    ));
}

fn wait_for(component: &mut moira_tui::PromptControlComponent, needle: &str) {
    for _ in 0..100 {
        component.poll();
        if render(component).contains(needle) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("component did not render {needle}");
}

fn render(component: &moira_tui::PromptControlComponent) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
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
                depth: d::SurfaceDepthV1::Activated,
            },
            persona: d::PersonaLensV1::Researcher,
        },
        revision: 14,
        phase: p::PhaseV1::Ready,
        session_revision: Some(15),
        last_event_cursor: None,
        profile_document_digest: Some([17; 32]),
        capabilities: p::CapabilitiesV1::default(),
        active_turn: None,
        permission: None,
        terminal: None,
        pending: None,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

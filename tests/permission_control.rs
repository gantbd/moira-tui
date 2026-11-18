use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{Sender, channel},
    },
    thread,
    time::Duration,
};

use moira_client::{AgentParticipationServicePort, AgentServicePort};
use moira_protocol::{WorkflowIdV1, agent, agent_participation as p, deliberation as d};
use ratatui::{Terminal, backend::TestBackend};
use sha2::{Digest, Sha256};

enum Mutation {
    Accept(Sender<p::ConfirmedPermissionSelectionV1>),
    Unknown(Arc<AtomicUsize>),
}

struct ParticipationPort {
    step: usize,
    mutation: Mutation,
}

impl AgentParticipationServicePort for ParticipationPort {
    type Error = ();

    fn execute_participation(&mut self, command: p::CommandV1) -> Result<p::ReplyV1, Self::Error> {
        let (view, _, _) = fixture();
        let confirmation = p::permission_confirmation(24, &view).unwrap();
        if let p::CommandV1::ConfirmPermission(selection) = command {
            return match &self.mutation {
                Mutation::Accept(sent) => {
                    sent.send(selection).unwrap();
                    Ok(p::ReplyV1::Accepted(Box::new(p::OutcomeV1::Applied {
                        storage_revision: 25,
                        participation: view,
                        replayed: false,
                    })))
                }
                Mutation::Unknown(calls) => {
                    calls.fetch_add(1, Ordering::Relaxed);
                    Err(())
                }
            };
        }
        let reply = if self.step.is_multiple_of(2) {
            assert_eq!(
                command,
                p::CommandV1::Participation {
                    participation: view.binding.participation
                }
            );
            p::OutcomeV1::Participation(Some(view))
        } else {
            assert_eq!(
                command,
                p::CommandV1::PermissionConfirmation {
                    participation: view.binding.participation
                }
            );
            p::OutcomeV1::PermissionConfirmation(Some(confirmation))
        };
        self.step += 1;
        Ok(p::ReplyV1::Accepted(Box::new(reply)))
    }
}

struct AgentReads {
    step: usize,
}

impl AgentServicePort for AgentReads {
    type Error = ();

    fn execute(&mut self, command: agent::CommandV1) -> Result<agent::ReplyV1, Self::Error> {
        let (_, session, request) = fixture();
        let outcome = match self.step {
            0 | 2 => {
                assert_eq!(
                    command,
                    agent::CommandV1::Inspect {
                        session: session.session
                    }
                );
                agent::OutcomeV1::Session(session)
            }
            1 => {
                assert_eq!(
                    command,
                    agent::CommandV1::Events {
                        session: session.session,
                        after: None,
                        limit: 64,
                    }
                );
                agent::OutcomeV1::Events(agent::EventPageV1 {
                    session: session.session,
                    entries: vec![agent::EventRecordV1 {
                        cursor: 0,
                        event: agent::EventV1::Permission(request),
                    }],
                    next_after: None,
                })
            }
            _ => panic!("unexpected agent read"),
        };
        self.step += 1;
        Ok(agent::ReplyV1::Accepted(outcome))
    }
}

#[test]
fn permission_requires_exact_correlated_option_then_arm_and_confirm() {
    let (sent, received) = channel();
    let mut component = moira_tui::PermissionControlComponent::new(
        ParticipationPort {
            step: 0,
            mutation: Mutation::Accept(sent),
        },
        AgentReads { step: 0 },
        p::ParticipationIdV1([1; 16]),
    );
    wait_for(&mut component, "allow\\x00once");
    let rendered = render(&component);
    assert!(rendered.contains("terminal: cargo test"));
    assert!(rendered.contains("> AllowOnce allow\\x00once"));
    assert!(rendered.contains("RejectOnce reject"));

    component.update(moira_tui::PermissionControlInput::Confirm, true);
    assert!(received.try_recv().is_err());
    component.update(moira_tui::PermissionControlInput::Down, true);
    component.update(moira_tui::PermissionControlInput::Arm, true);
    assert!(render(&component).contains("ARMED"));
    component.update(moira_tui::PermissionControlInput::Confirm, true);

    let selected = received.recv_timeout(Duration::from_secs(1)).unwrap();
    let (view, _, request) = fixture();
    let confirmation = p::permission_confirmation(24, &view).unwrap();
    assert_eq!(selected.participation, view.binding.participation);
    assert_eq!(
        selected.confirmation_digest,
        confirmation.confirmation_digest
    );
    assert_eq!(selected.option.0, request.options[1].id.0);
    wait_for(&mut component, "Permission choice durably accepted");
}

#[test]
fn unknown_permission_outcome_fences_another_confirmation_until_refresh() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut component = moira_tui::PermissionControlComponent::new(
        ParticipationPort {
            step: 0,
            mutation: Mutation::Unknown(Arc::clone(&calls)),
        },
        AgentReads { step: 0 },
        p::ParticipationIdV1([1; 16]),
    );
    wait_for(&mut component, "server challenge:");
    component.update(moira_tui::PermissionControlInput::Arm, true);
    component.update(moira_tui::PermissionControlInput::Confirm, true);
    wait_for(&mut component, "OUTCOME UNKNOWN");
    assert_eq!(calls.load(Ordering::Relaxed), 1);

    component.update(moira_tui::PermissionControlInput::Arm, true);
    component.update(moira_tui::PermissionControlInput::Confirm, true);
    thread::sleep(Duration::from_millis(5));
    component.poll();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

fn wait_for(component: &mut moira_tui::PermissionControlComponent, needle: &str) {
    for _ in 0..100 {
        component.poll();
        if render(component).contains(needle) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("component did not render {needle}");
}

fn render(component: &moira_tui::PermissionControlComponent) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 22)).unwrap();
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

fn fixture() -> (
    p::ParticipationViewV1,
    agent::SessionViewV1,
    agent::PermissionRequestV1,
) {
    let options = vec![
        agent::PermissionOptionV1 {
            id: agent::PermissionOptionIdV1([21; 16]),
            kind: agent::PermissionOptionKindV1::AllowOnce,
            label: b"allow\0once".to_vec(),
        },
        agent::PermissionOptionV1 {
            id: agent::PermissionOptionIdV1([22; 16]),
            kind: agent::PermissionOptionKindV1::RejectOnce,
            label: b"reject".to_vec(),
        },
    ];
    let request = agent::PermissionRequestV1 {
        request: agent::PermissionRequestIdV1([19; 16]),
        turn: agent::TurnIdV1([18; 16]),
        capability: b"terminal: cargo test".to_vec(),
        description: b"run tests".to_vec(),
        options: options.clone(),
    };
    let mut option_ids = [p::PermissionOptionIdV1([0; 16]); p::MAX_PERMISSION_OPTIONS_V1];
    option_ids[0] = p::PermissionOptionIdV1(options[0].id.0);
    option_ids[1] = p::PermissionOptionIdV1(options[1].id.0);
    let view = p::ParticipationViewV1 {
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
            persona: d::PersonaLensV1::RigorReviewer,
        },
        revision: 14,
        phase: p::PhaseV1::AwaitingPermission,
        session_revision: Some(15),
        last_event_cursor: Some(0),
        profile_document_digest: Some([17; 32]),
        capabilities: p::CapabilitiesV1 {
            permissions: true,
            ..p::CapabilitiesV1::default()
        },
        active_turn: Some(p::AgentTurnIdV1(request.turn.0)),
        permission: Some(p::PermissionEvidenceV1 {
            request: p::PermissionRequestIdV1(request.request.0),
            turn: p::AgentTurnIdV1(request.turn.0),
            capability_digest: Sha256::digest(&request.capability).into(),
            options_digest: options_digest(&options),
            option_count: 2,
            option_ids,
        }),
        terminal: None,
        pending: None,
    };
    let session = agent::SessionViewV1 {
        session: agent::SessionIdV1(view.binding.session.0),
        revision: 15,
        phase: agent::PhaseV1::AwaitingPermission,
        profile_document_digest: agent::DigestV1([17; 32]),
        active_turn: Some(request.turn),
        last_event_cursor: Some(0),
        last_context: None,
        capabilities: agent::CapabilitiesV1 {
            permissions: true,
            ..agent::CapabilitiesV1::default()
        },
    };
    (view, session, request)
}

fn options_digest(values: &[agent::PermissionOptionV1]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"MOIRAOPT1");
    hash.update((values.len() as u64).to_be_bytes());
    for value in values {
        hash.update(value.id.0);
        hash.update([match value.kind {
            agent::PermissionOptionKindV1::AllowOnce => 0,
            agent::PermissionOptionKindV1::AllowAlways => 1,
            agent::PermissionOptionKindV1::RejectOnce => 2,
            agent::PermissionOptionKindV1::RejectAlways => 3,
        }]);
        hash.update((value.label.len() as u64).to_be_bytes());
        hash.update(&value.label);
    }
    hash.finalize().into()
}

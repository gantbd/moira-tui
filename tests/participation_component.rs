use std::{thread, time::Duration};

use moira_client::AgentParticipationServicePort;
use moira_protocol::{WorkflowIdV1, agent_participation as p, deliberation as d};
use ratatui::{Terminal, backend::TestBackend};

struct Port;

impl AgentParticipationServicePort for Port {
    type Error = ();

    fn execute_participation(&mut self, command: p::CommandV1) -> Result<p::ReplyV1, Self::Error> {
        assert_eq!(
            command,
            p::CommandV1::Participation {
                participation: p::ParticipationIdV1([1; 16])
            }
        );
        Ok(p::ReplyV1::Accepted(Box::new(p::OutcomeV1::Participation(
            Some(view()),
        ))))
    }
}

struct SequencePort {
    observations: Vec<(u64, d::PersonaLensV1)>,
    next: usize,
}

impl AgentParticipationServicePort for SequencePort {
    type Error = ();

    fn execute_participation(&mut self, _: p::CommandV1) -> Result<p::ReplyV1, Self::Error> {
        let (revision, persona) = self.observations[self.next.min(self.observations.len() - 1)];
        self.next += 1;
        let mut value = view();
        value.revision = revision;
        value.binding.persona = persona;
        Ok(p::ReplyV1::Accepted(Box::new(p::OutcomeV1::Participation(
            Some(value),
        ))))
    }
}

#[test]
fn component_renders_only_the_public_governed_projection() {
    let mut component = moira_tui::ParticipationComponent::new(Port, p::ParticipationIdV1([1; 16]));
    for _ in 0..100 {
        component.poll();
        let rendered = render(&component);
        if rendered.contains("AwaitingPermission") {
            assert!(rendered.contains("participation revision: 14"));
            assert!(rendered.contains("principal type: agent"));
            assert!(rendered.contains("accountable owner: Moira"));
            assert!(rendered.contains("participation: 01010101010101010101010101010101"));
            assert!(rendered.contains("workflow: 02020202020202020202020202020202"));
            assert!(
                rendered
                    .contains("task: 07070707070707070707070707070707 · incarnation 8 · attempt 9")
            );
            assert!(rendered.contains("persona: Historian"));
            assert!(rendered.contains("grounding depth: Reference"));
            assert!(rendered.contains("grounding authority: 0c0c0c0c"));
            assert!(rendered.contains("method machine: 0a0a0a0a"));
            assert!(rendered.contains("profile document: 11111111"));
            assert!(rendered.contains("permission: 2 exact option(s)"));
            assert!(rendered.contains("pending operation: none"));
            assert!(rendered.contains("permission=true read=true write=false terminal=true"));
            assert!(rendered.contains("load=true resume=true modes=true models=true"));
            assert!(
                rendered.contains("not published: role/work mode · scope · delegation · budgets")
            );
            let agenda = component
                .operator_agenda_observation()
                .expect("current participation contributes an agenda observation");
            assert_eq!(agenda.revision, 14);
            assert_eq!(agenda.phase, "AwaitingPermission");
            assert_eq!(agenda.permission_option_count, Some(2));
            assert_eq!(agenda.pending_operation, None);
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("participation component did not render its accepted projection");
}

#[test]
fn component_orients_revision_advances_and_rejects_owner_regression_or_rewrite() {
    let port = SequencePort {
        observations: vec![
            (14, d::PersonaLensV1::Historian),
            (15, d::PersonaLensV1::Historian),
            (14, d::PersonaLensV1::Historian),
            (15, d::PersonaLensV1::Researcher),
        ],
        next: 0,
    };
    let mut component = moira_tui::ParticipationComponent::new(port, p::ParticipationIdV1([1; 16]));

    wait_for(&mut component, "Current governed revision 14");
    let _ = component.update(moira_tui::Input::Refresh, true);
    let advanced = wait_for(&mut component, "Governed revision advanced 14 → 15");
    assert!(advanced.contains("participation revision: 15"));
    assert!(advanced.contains("previously observed revision: 14"));

    let _ = component.update(moira_tui::Input::Refresh, true);
    let regressed = wait_for(&mut component, "Moira revision regressed 15 → 14");
    assert!(regressed.contains("participation revision: 15"));

    let _ = component.update(moira_tui::Input::Refresh, true);
    let rewritten = wait_for(&mut component, "Moira changed revision 15 in place");
    assert!(rewritten.contains("persona: Historian"));
    assert!(!rewritten.contains("persona: Researcher"));
}

fn wait_for(component: &mut moira_tui::ParticipationComponent, expected: &str) -> String {
    for _ in 0..100 {
        component.poll();
        let rendered = render(component);
        if rendered.contains(expected) {
            return rendered;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("participation component did not render {expected:?}");
}

fn render(component: &moira_tui::ParticipationComponent) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 32)).unwrap();
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
    let turn = p::AgentTurnIdV1([19; 16]);
    let mut option_ids = [p::PermissionOptionIdV1([0; 16]); p::MAX_PERMISSION_OPTIONS_V1];
    option_ids[0] = p::PermissionOptionIdV1([20; 16]);
    option_ids[1] = p::PermissionOptionIdV1([21; 16]);
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
        phase: p::PhaseV1::AwaitingPermission,
        session_revision: Some(15),
        last_event_cursor: Some(16),
        profile_document_digest: Some([17; 32]),
        capabilities: p::CapabilitiesV1 {
            load_session: true,
            resume_session: true,
            permissions: true,
            filesystem_read: true,
            terminal: true,
            session_modes: true,
            session_models: true,
            ..p::CapabilitiesV1::default()
        },
        active_turn: Some(turn),
        permission: Some(p::PermissionEvidenceV1 {
            request: p::PermissionRequestIdV1([18; 16]),
            turn,
            capability_digest: [22; 32],
            options_digest: [23; 32],
            option_count: 2,
            option_ids,
        }),
        terminal: None,
        pending: None,
    }
}

use std::{thread, time::Duration};

use moira_client::DeliberationServicePort;
use moira_protocol::deliberation as d;
use ratatui::{Terminal, backend::TestBackend};

struct Port {
    step: usize,
    stale: bool,
}

impl DeliberationServicePort for Port {
    type Error = ();

    fn execute(&mut self, command: d::CommandV1) -> Result<d::ReplyV1, Self::Error> {
        let response = match self.step % 3 {
            0 | 2 => {
                assert_eq!(command, d::CommandV1::Status);
                d::OutcomeV1::Status(d::StatusV1 {
                    revision: if self.stale && self.step % 3 == 2 {
                        8
                    } else {
                        7
                    },
                    needs_reopen: false,
                })
            }
            1 => {
                assert_eq!(
                    command,
                    d::CommandV1::Deliberation {
                        deliberation: d::DeliberationIdV1([1; 16])
                    }
                );
                d::OutcomeV1::Deliberation(Some(Box::new(view())))
            }
            _ => unreachable!(),
        };
        self.step += 1;
        Ok(d::ReplyV1::Accepted(response))
    }
}

#[test]
fn component_renders_persona_dissent_candidates_and_retained_choice() {
    let mut component = moira_tui::DeliberationComponent::new(
        Port {
            step: 0,
            stale: false,
        },
        d::DeliberationIdV1([1; 16]),
    );
    for _ in 0..100 {
        component.poll();
        let rendered = render(&component);
        if rendered.contains("Stable registry revision 7") {
            assert!(rendered.contains("phase: Resolved"));
            assert!(rendered.contains("right ensemble? Contested"));
            assert!(rendered.contains("ensemble coverage:"));
            assert!(rendered.contains("ensemble equity:"));
            assert!(rendered.contains("persona Historian"));
            assert!(rendered.contains("stakeholder TechnicalSme"));
            assert!(rendered.contains("stakeholder standing: accountable=none"));
            assert!(rendered.contains("evidence=2 dissent=1"));
            assert!(rendered.contains("candidate 07070707"));
            assert!(rendered.contains("resolution: authorized choice"));
            assert!(rendered.contains("human gate: changes_requested"));
            assert!(rendered.contains("human gate material:"));
            assert!(rendered.contains("human gate authority:"));
            assert!(rendered.contains("no frontier or authority inferred"));
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("deliberation component did not render its stable public projection");
}

#[test]
fn component_rejects_a_view_observed_across_registry_revision_change() {
    let mut component = moira_tui::DeliberationComponent::new(
        Port {
            step: 0,
            stale: true,
        },
        d::DeliberationIdV1([1; 16]),
    );
    for _ in 0..100 {
        component.poll();
        let rendered = render(&component);
        if rendered.contains("changed during observation") {
            assert!(rendered.contains("No stable deliberation observation"));
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("component admitted a mixed-revision deliberation");
}

#[test]
fn reusable_projection_preserves_contested_admission_and_stakeholder_standing() {
    let value = moira_tui::deliberation_view_json(view());
    assert_eq!(value["admission"]["fitness"], "contested");
    assert_eq!(
        value["members"][0]["stakeholder"]["representation"],
        "technical_sme"
    );
    assert_eq!(
        value["members"][0]["stakeholder"]["accountable_principal"],
        serde_json::Value::Null
    );
    assert_eq!(value["human_gate"]["status"], "settled");
    assert_eq!(
        value["human_gate"]["response"]["disposition"],
        "changes_requested"
    );
    assert_eq!(
        value["human_gate"]["request"]["material_revision_digest"],
        "2525252525252525252525252525252525252525252525252525252525252525"
    );

    let mut pending = view();
    pending.human_gate_response = None;
    let pending = moira_tui::deliberation_view_json(pending);
    assert_eq!(pending["human_gate"]["status"], "pending");
    assert_eq!(pending["human_gate"]["response"], serde_json::Value::Null);
}

fn render(component: &moira_tui::DeliberationComponent) -> String {
    let mut terminal = Terminal::new(TestBackend::new(180, 40)).unwrap();
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

fn view() -> d::DeliberationViewV1 {
    d::DeliberationViewV1 {
        version: 1,
        plan: d::DeliberationPlanV1 {
            id: d::DeliberationIdV1([1; 16]),
            question_digest: [2; 32],
            admission: d::EnsembleAdmissionV1 {
                affected_population_digest: [31; 32],
                stakeholder_map_digest: [32; 32],
                coverage_manifest_digest: [33; 32],
                equity_assessment_digest: [34; 32],
                assessor_principal: [35; 16],
                fitness: d::EnsembleFitnessV1::Contested,
            },
            method: d::MethodRefV1 {
                machine_digest: [3; 32],
                state_digest: [4; 32],
            },
            claims: vec![d::ClaimKindV1::Historical],
            criteria: vec![d::CriterionV1 {
                id: d::CriterionIdV1([5; 16]),
                direction: d::CriterionDirectionV1::Maximize,
            }],
            candidates: vec![d::CandidateV1 {
                id: d::CandidateIdV1([7; 16]),
                scores: vec![8],
            }],
            members: vec![d::EnsembleMemberV1 {
                id: d::EnsembleMemberIdV1([9; 16]),
                stakeholder: d::StakeholderRefV1::technical_sme([29; 16], [30; 32]),
                lens: d::PersonaLensV1::Historian,
                concern_root_digest: [10; 32],
                grounding: d::GroundingRefV1 {
                    authority_digest: [11; 32],
                    surface_digest: [12; 32],
                    depth: d::SurfaceDepthV1::Activated,
                },
            }],
            orthogonality: d::OrthogonalityEvidenceV1 {
                query_digest: [13; 32],
                result_digest: [14; 32],
                all_orthogonal: true,
            },
            fuel: 2,
        },
        limits: d::DeliberationLimitsV1 {
            max_claims: 4,
            max_criteria: 4,
            max_candidates: 4,
            max_members: 4,
            max_evidence_per_report: 4,
            max_dissent_per_report: 4,
            max_fuel: 4,
        },
        reports: vec![d::MemberReportV1 {
            id: d::ReportIdV1([15; 16]),
            member: d::EnsembleMemberIdV1([9; 16]),
            output_digest: [16; 32],
            evidence_digests: vec![[17; 32], [18; 32]],
            dissent_digests: vec![[19; 32]],
        }],
        resolution: Some(d::ResolutionV1::AuthorizedChoice {
            frontier: vec![d::CandidateIdV1([7; 16])],
            selected: d::CandidateIdV1([7; 16]),
            authority: d::AuthorityRefV1 {
                decision: d::AuthorityDecisionIdV1([20; 16]),
                subject: d::AuthoritySubjectIdV1([21; 16]),
                epoch: 22,
            },
        }),
        human_gate_request: Some(d::HumanGateRequestV1 {
            id: d::HumanGateRequestIdV1([23; 16]),
            reviewer: [24; 16],
            finding_digest: [35; 32],
            decision_digest: [36; 32],
            material_revision_digest: [37; 32],
            policy_digest: [38; 32],
            requested_at: 39,
        }),
        human_gate_response: Some(d::HumanGateResponseV1 {
            id: d::HumanGateResponseIdV1([40; 16]),
            request: d::HumanGateRequestIdV1([23; 16]),
            disposition: d::HumanGateDispositionV1::ChangesRequested,
            rationale_digest: [41; 32],
            responded_at: 42,
            authority: d::HumanGateAuthorityRefV1 {
                decision: [43; 16],
                subject: [24; 16],
                epoch: 44,
            },
        }),
        remaining_fuel: 1,
        phase: d::DeliberationPhaseV1::Resolved,
    }
}

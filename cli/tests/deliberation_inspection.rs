#![cfg(unix)]

use moira_core::deliberation::{
    DeliberationLimitsV1,
    registry::{RegistryLimitsV1, persistence::CodecLimitsV1},
};
use moira_local_daemon::unix::{Authenticator, DeliberationDaemon};
use moira_protocol::deliberation::*;
use moira_service::{
    AccessV1,
    deliberation::{DeliberationService, HumanGateAccessV1},
};
use moira_sqlite::deliberation::{DeliberationSqliteStore, DeliberationStoreLimitsV1};
use moira_wire::LimitsV1;
use ratatui::{Terminal, backend::TestBackend};
use serde_json::Value;
use std::{
    convert::Infallible,
    fs,
    os::unix::net::UnixStream,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct ReadOnly;

impl Authenticator for ReadOnly {
    type Error = Infallible;

    fn authenticate(&mut self, _: &UnixStream) -> Result<AccessV1, Self::Error> {
        Ok(AccessV1 {
            read: true,
            mutate: false,
        })
    }
}

struct Fixture {
    root: PathBuf,
    database: PathBuf,
    socket: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "moira-tui-deliberation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self {
            database: root.join("deliberation.db"),
            socket: root.join("deliberation.sock"),
            root,
        }
    }

    fn open(&self) -> DeliberationSqliteStore {
        DeliberationSqliteStore::open(
            &self.database,
            [31; 16],
            DeliberationStoreLimitsV1 {
                codec: CodecLimitsV1 {
                    max_bytes: 65_536,
                    max_deliberation_bytes: 32_768,
                    registry: RegistryLimitsV1 {
                        max_deliberations: 8,
                        deliberation: DeliberationLimitsV1 {
                            max_claims: 8,
                            max_criteria: 8,
                            max_candidates: 8,
                            max_members: 4,
                            max_evidence_per_report: 8,
                            max_dissent_per_report: 8,
                            max_fuel: 32,
                        },
                    },
                },
                max_database_pages: 64,
            },
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
        for name in [
            "deliberation.db",
            "deliberation.db-journal",
            "deliberation.db.owner.lock",
        ] {
            let _ = fs::remove_file(self.root.join(name));
        }
        let _ = fs::remove_dir(&self.root);
    }
}

#[test]
fn headless_cli_observes_exact_methodical_facts_through_the_read_only_daemon() {
    let fixture = Fixture::new();
    {
        let mut service = DeliberationService::new(fixture.open());
        apply(&mut service, 0, MutationV1::Admit(Box::new(plan())), 1);
        apply(
            &mut service,
            1,
            MutationV1::SubmitReport {
                deliberation: DeliberationIdV1([1; 16]),
                report: report(),
            },
            2,
        );
        apply(
            &mut service,
            2,
            MutationV1::Resolve {
                deliberation: DeliberationIdV1([1; 16]),
                resolution: ResolutionV1::AuthorizedChoice {
                    frontier: vec![CandidateIdV1([5; 16])],
                    selected: CandidateIdV1([5; 16]),
                    authority: AuthorityRefV1 {
                        decision: AuthorityDecisionIdV1([24; 16]),
                        subject: AuthoritySubjectIdV1([25; 16]),
                        epoch: 26,
                    },
                },
            },
            3,
        );
        apply(
            &mut service,
            3,
            MutationV1::RequestHumanGate {
                deliberation: DeliberationIdV1([1; 16]),
                request: HumanGateRequestV1 {
                    id: HumanGateRequestIdV1([27; 16]),
                    reviewer: [28; 16],
                    finding_digest: [29; 32],
                    decision_digest: [30; 32],
                    material_revision_digest: [31; 32],
                    policy_digest: [32; 32],
                    requested_at: 33,
                },
            },
            4,
        );
        assert!(matches!(
            service
                .execute_human_gate_authorized(
                    AccessV1 {
                        read: true,
                        mutate: true,
                    },
                    HumanGateAccessV1 {
                        decision: [34; 16],
                        subject: [28; 16],
                        epoch: 35,
                    },
                    HumanGateOperationV1 {
                        deliberation: DeliberationIdV1([1; 16]),
                        expected_revision: 4,
                        response: HumanGateResponseSubmissionV1 {
                            id: HumanGateResponseIdV1([36; 16]),
                            request: HumanGateRequestIdV1([27; 16]),
                            disposition: HumanGateDispositionV1::Deferred,
                            rationale_digest: [37; 32],
                            responded_at: 38,
                        },
                    },
                )
                .unwrap(),
            ReplyV1::Accepted(OutcomeV1::Applied {
                revision: 5,
                replayed: false,
                ..
            })
        ));
    }
    let mut daemon = DeliberationDaemon::bind_new(
        &fixture.socket,
        DeliberationService::new(fixture.open()),
        ReadOnly,
        LimitsV1::default(),
        Duration::from_secs(3),
        8,
    )
    .unwrap();
    let worker = thread::spawn(move || daemon.serve_next().unwrap());
    let output = Command::new(env!("CARGO_BIN_EXE_moira-tui"))
        .args([
            "deliberation-inspect",
            fixture.socket.to_str().unwrap(),
            "01010101010101010101010101010101",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["capability"], "deliberation.inspect.v1");
    assert_eq!(value["status"], "current");
    assert_eq!(value["registry_revision"], 5);
    assert_eq!(value["deliberation"]["phase"], "resolved");
    assert_eq!(value["deliberation"]["admission"]["fitness"], "yes");
    assert_eq!(
        value["deliberation"]["admission"]["assessor_principal"],
        "19191919191919191919191919191919"
    );
    assert_eq!(value["deliberation"]["members"][0]["persona"], "historian");
    assert_eq!(
        value["deliberation"]["members"][0]["stakeholder"]["representation"],
        "technical_sme"
    );
    assert_eq!(
        value["deliberation"]["members"][0]["stakeholder"]["principal"],
        "1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a"
    );
    assert_eq!(
        value["deliberation"]["reports"][0]["dissent_digests"][0],
        "1717171717171717171717171717171717171717171717171717171717171717"
    );
    assert_eq!(
        value["deliberation"]["resolution"]["kind"],
        "authorized_choice"
    );
    assert_eq!(
        value["deliberation"]["resolution"]["frontier"][0],
        "05050505050505050505050505050505"
    );
    assert_eq!(value["deliberation"]["human_gate"]["status"], "settled");
    assert_eq!(
        value["deliberation"]["human_gate"]["response"]["disposition"],
        "deferred"
    );
    assert_eq!(
        value["deliberation"]["human_gate"]["response"]["authority"]["subject"],
        "1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c"
    );
    worker.join().unwrap();

    let mut daemon = DeliberationDaemon::bind_new(
        &fixture.socket,
        DeliberationService::new(fixture.open()),
        ReadOnly,
        LimitsV1::default(),
        Duration::from_secs(3),
        8,
    )
    .unwrap();
    let worker = thread::spawn(move || daemon.serve_next().unwrap());
    let port = moira_tui_local_client::LocalDeliberationPort::connect(
        &fixture.socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let mut component = moira_tui::DeliberationComponent::new(port, DeliberationIdV1([1; 16]));
    for _ in 0..100 {
        component.poll();
        let rendered = render(&component);
        if rendered.contains("Stable registry revision 5") {
            assert!(rendered.contains("right ensemble? Yes"));
            assert!(rendered.contains("persona Historian"));
            assert!(rendered.contains("stakeholder TechnicalSme"));
            assert!(rendered.contains("evidence=1 dissent=1"));
            assert!(rendered.contains("resolution: authorized choice"));
            assert!(rendered.contains("human gate: deferred"));
            assert!(rendered.contains("human gate material:"));
            assert!(rendered.contains("human gate authority:"));
            drop(component);
            worker.join().unwrap();
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("real-daemon deliberation component did not render");
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

fn apply(
    service: &mut DeliberationService<DeliberationSqliteStore>,
    expected_revision: u64,
    mutation: MutationV1,
    revision: u64,
) {
    assert!(matches!(
        service
            .execute(CommandV1::Apply {
                expected_revision,
                mutation,
            })
            .unwrap(),
        ReplyV1::Accepted(OutcomeV1::Applied {
            revision: actual,
            replayed: false,
            ..
        }) if actual == revision
    ));
}

fn plan() -> DeliberationPlanV1 {
    DeliberationPlanV1 {
        id: DeliberationIdV1([1; 16]),
        admission: moira_protocol::deliberation::EnsembleAdmissionV1::affirmed(
            [21; 32], [22; 32], [23; 32], [24; 32], [25; 16],
        ),
        question_digest: [20; 32],
        method: MethodRefV1 {
            machine_digest: [2; 32],
            state_digest: [3; 32],
        },
        claims: vec![ClaimKindV1::Historical],
        criteria: vec![CriterionV1 {
            id: CriterionIdV1([4; 16]),
            direction: CriterionDirectionV1::Maximize,
        }],
        candidates: vec![CandidateV1 {
            id: CandidateIdV1([5; 16]),
            scores: vec![1],
        }],
        members: vec![EnsembleMemberV1 {
            id: EnsembleMemberIdV1([6; 16]),
            lens: PersonaLensV1::Historian,
            stakeholder: moira_protocol::deliberation::StakeholderRefV1::technical_sme(
                [26; 16], [27; 32],
            ),
            concern_root_digest: [7; 32],
            grounding: GroundingRefV1 {
                authority_digest: [8; 32],
                surface_digest: [9; 32],
                depth: SurfaceDepthV1::Activated,
            },
        }],
        orthogonality: OrthogonalityEvidenceV1 {
            query_digest: [10; 32],
            result_digest: [11; 32],
            all_orthogonal: true,
        },
        fuel: 2,
    }
}

fn report() -> MemberReportV1 {
    MemberReportV1 {
        id: ReportIdV1([21; 16]),
        member: EnsembleMemberIdV1([6; 16]),
        output_digest: [22; 32],
        evidence_digests: vec![[16; 32]],
        dissent_digests: vec![[23; 32]],
    }
}

#![cfg(unix)]

use std::{convert::Infallible, os::unix::net::UnixStream, path::PathBuf, time::Duration};

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let socket = PathBuf::from(args.next().ok_or("missing socket")?);
    let database = PathBuf::from(args.next().ok_or("missing database")?);
    let mode = args.next();
    let pending = mode.is_some();
    let decision_packet = match mode.as_deref() {
        None => HumanGateMaterialRefV1 {
            owner_route: [60; 16],
            artifact_route: [61; 16],
            expected_revision: 62,
            expected_sha256: [63; 32],
        },
        Some(value) if value == "pending" => HumanGateMaterialRefV1 {
            owner_route: [60; 16],
            artifact_route: [61; 16],
            expected_revision: 62,
            expected_sha256: [63; 32],
        },
        Some(value) if value == "pending-packet" => {
            let revision = args
                .next()
                .ok_or("pending-packet requires a revision")?
                .to_string_lossy()
                .parse::<u64>()?;
            let digest = args
                .next()
                .ok_or("pending-packet requires a SHA-256 digest")?;
            HumanGateMaterialRefV1 {
                owner_route: [60; 16],
                artifact_route: [61; 16],
                expected_revision: revision,
                expected_sha256: parse_digest(&digest.to_string_lossy())?,
            }
        }
        Some(_) => return Err("third argument must be pending or pending-packet".into()),
    };
    if args.next().is_some() {
        return Err(
            "usage: deliberation_fixture SOCKET DATABASE [pending|pending-packet REVISION SHA256]"
                .into(),
        );
    }
    let mut store = open(&database)?;
    if store.revision().0 == 0 {
        let mut service = DeliberationService::new(store);
        apply(&mut service, 0, MutationV1::Admit(Box::new(plan())), 1)?;
        apply(
            &mut service,
            1,
            MutationV1::SubmitReport {
                deliberation: DeliberationIdV1([1; 16]),
                report: MemberReportV1 {
                    id: ReportIdV1([41; 16]),
                    member: EnsembleMemberIdV1([31; 16]),
                    output_digest: [42; 32],
                    evidence_digests: vec![[43; 32]],
                    dissent_digests: vec![[44; 32]],
                },
            },
            2,
        )?;
        apply(
            &mut service,
            2,
            MutationV1::Resolve {
                deliberation: DeliberationIdV1([1; 16]),
                resolution: ResolutionV1::AuthorizedChoice {
                    frontier: vec![CandidateIdV1([29; 16])],
                    selected: CandidateIdV1([29; 16]),
                    authority: AuthorityRefV1 {
                        decision: AuthorityDecisionIdV1([45; 16]),
                        subject: AuthoritySubjectIdV1([46; 16]),
                        epoch: 47,
                    },
                },
            },
            3,
        )?;
        apply(
            &mut service,
            3,
            MutationV1::RequestHumanGate {
                deliberation: DeliberationIdV1([1; 16]),
                request: HumanGateRequestV1 {
                    id: HumanGateRequestIdV1([48; 16]),
                    reviewer: [49; 16],
                    finding_digest: [50; 32],
                    decision_digest: [51; 32],
                    material_revision_digest: [52; 32],
                    policy_digest: [53; 32],
                    decision_packet,
                    requested_at: 54,
                },
            },
            4,
        )?;
        if !pending {
            match service.execute_human_gate_authorized(
                AccessV1 {
                    read: true,
                    mutate: true,
                },
                HumanGateAccessV1 {
                    decision: [55; 16],
                    subject: [49; 16],
                    epoch: 56,
                },
                HumanGateOperationV1 {
                    deliberation: DeliberationIdV1([1; 16]),
                    expected_revision: 4,
                    response: HumanGateResponseSubmissionV1 {
                        id: HumanGateResponseIdV1([57; 16]),
                        request: HumanGateRequestIdV1([48; 16]),
                        disposition: HumanGateDispositionV1::ChangesRequested,
                        rationale_digest: [58; 32],
                        rationale: HumanGateMaterialRefV1 {
                            owner_route: [64; 16],
                            artifact_route: [65; 16],
                            expected_revision: 66,
                            expected_sha256: [67; 32],
                        },
                        responded_at: 59,
                    },
                },
            )? {
                ReplyV1::Accepted(OutcomeV1::Applied {
                    revision: 5,
                    replayed: false,
                    ..
                }) => {}
                reply => return Err(format!("unexpected fixture gate reply: {reply:?}").into()),
            }
        }
        store = service.into_store();
    }
    let mut daemon = DeliberationDaemon::bind_new(
        &socket,
        DeliberationService::new(store),
        ReadOnly,
        LimitsV1::default(),
        Duration::from_secs(5),
        64,
    )?;
    println!("READY {}", socket.display());
    loop {
        daemon
            .serve_next()
            .map_err(|error| format!("deliberation daemon failed: {error:?}"))?;
    }
}

fn parse_digest(value: &str) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    if value.len() != 64 || !value.is_ascii() {
        return Err("SHA-256 digest must be 64 lowercase hexadecimal digits".into());
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        let pair = &value[index * 2..index * 2 + 2];
        if !pair
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
        {
            return Err("SHA-256 digest must be 64 lowercase hexadecimal digits".into());
        }
        *byte = u8::from_str_radix(pair, 16)?;
    }
    Ok(digest)
}

fn open(database: &std::path::Path) -> Result<DeliberationSqliteStore, Box<dyn std::error::Error>> {
    Ok(DeliberationSqliteStore::open(
        database,
        [61; 16],
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
    )?)
}

fn plan() -> DeliberationPlanV1 {
    DeliberationPlanV1 {
        id: DeliberationIdV1([1; 16]),
        question_digest: [20; 32],
        admission: EnsembleAdmissionV1 {
            affected_population_digest: [21; 32],
            stakeholder_map_digest: [22; 32],
            coverage_manifest_digest: [23; 32],
            equity_assessment_digest: [24; 32],
            assessor_principal: [25; 16],
            fitness: EnsembleFitnessV1::Yes,
        },
        method: MethodRefV1 {
            machine_digest: [26; 32],
            state_digest: [27; 32],
        },
        claims: vec![ClaimKindV1::Empirical, ClaimKindV1::Normative],
        criteria: vec![CriterionV1 {
            id: CriterionIdV1([28; 16]),
            direction: CriterionDirectionV1::Maximize,
        }],
        candidates: vec![CandidateV1 {
            id: CandidateIdV1([29; 16]),
            scores: vec![1],
        }],
        members: vec![EnsembleMemberV1 {
            id: EnsembleMemberIdV1([31; 16]),
            stakeholder: StakeholderRefV1 {
                principal: [32; 16],
                constituency_digest: [33; 32],
                representation: StakeholderRepresentationV1::AuthorizedRepresentative,
                accountable_principal: Some([34; 16]),
                mandate_digest: Some([35; 32]),
                conflict_digest: Some([36; 32]),
            },
            lens: PersonaLensV1::RigorReviewer,
            concern_root_digest: [37; 32],
            grounding: GroundingRefV1 {
                authority_digest: [38; 32],
                surface_digest: [39; 32],
                depth: SurfaceDepthV1::Activated,
            },
        }],
        orthogonality: OrthogonalityEvidenceV1 {
            query_digest: [40; 32],
            result_digest: [41; 32],
            all_orthogonal: true,
        },
        fuel: 3,
    }
}

fn apply(
    service: &mut DeliberationService<DeliberationSqliteStore>,
    expected_revision: u64,
    mutation: MutationV1,
    revision: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    match service.execute(CommandV1::Apply {
        expected_revision,
        mutation,
    })? {
        ReplyV1::Accepted(OutcomeV1::Applied {
            revision: actual,
            replayed: false,
            ..
        }) if actual == revision => Ok(()),
        reply => Err(format!("unexpected fixture reply: {reply:?}").into()),
    }
}

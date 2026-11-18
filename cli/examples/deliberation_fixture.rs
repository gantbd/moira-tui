#![cfg(unix)]

use std::{convert::Infallible, os::unix::net::UnixStream, path::PathBuf, time::Duration};

use moira_core::deliberation::{
    DeliberationLimitsV1,
    registry::{RegistryLimitsV1, persistence::CodecLimitsV1},
};
use moira_local_daemon::unix::{Authenticator, DeliberationDaemon};
use moira_protocol::deliberation::*;
use moira_service::{AccessV1, deliberation::DeliberationService};
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
    if args.next().is_some() {
        return Err("usage: deliberation_fixture SOCKET DATABASE".into());
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

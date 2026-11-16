#![cfg(unix)]

use std::{
    convert::Infallible,
    fs,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use moira_core::agent_participation::{LimitsV1 as DomainLimits, persistence::CodecLimitsV1};
use moira_host::AgentGateway;
use moira_local_daemon::unix::{
    AgentDaemon, AgentParticipationAuthenticator, AgentParticipationDaemon, Authenticator,
};
use moira_protocol::{WorkflowIdV1, agent, agent_participation as p, deliberation as d};
use moira_service::{
    AccessV1,
    agent::AgentService,
    agent_participation::{AgentParticipationService, PermissionAccessV1},
};
use moira_sqlite::agent_participation::{
    AgentParticipationSqliteStore, AgentParticipationStoreLimitsV1,
};
use moira_wire::LimitsV1;
use serde_json::Value;
use sha2::{Digest, Sha256};

static NEXT: AtomicU64 = AtomicU64::new(0);
static TEST_LOCK: Mutex<()> = Mutex::new(());

struct ParticipationAuthentication {
    calls: Arc<AtomicUsize>,
    mutate: bool,
}

impl AgentParticipationAuthenticator for ParticipationAuthentication {
    type Error = Infallible;

    fn authenticate(&mut self, _: &UnixStream) -> Result<AccessV1, Self::Error> {
        Ok(AccessV1 {
            read: true,
            mutate: self.mutate,
        })
    }

    fn authorize_permission(
        &mut self,
        selection: p::PermissionSelectionV1,
    ) -> Result<Option<PermissionAccessV1>, Self::Error> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(Some(PermissionAccessV1 {
            decision: [41; 16],
            subject: [42; 16],
            capability_digest: selection.capability_digest,
            epoch: 43,
        }))
    }
}

struct ReadAuthentication;

impl Authenticator for ReadAuthentication {
    type Error = Infallible;

    fn authenticate(&mut self, _: &UnixStream) -> Result<AccessV1, Self::Error> {
        Ok(AccessV1 {
            read: true,
            mutate: false,
        })
    }
}

#[derive(Clone)]
struct Gateway {
    session: agent::SessionViewV1,
    permission: agent::PermissionRequestV1,
}

impl AgentGateway for Gateway {
    type Error = Infallible;

    fn execute_agent(&mut self, command: agent::CommandV1) -> Result<agent::ReplyV1, Self::Error> {
        let outcome = match command {
            agent::CommandV1::Inspect { session } if session == self.session.session => {
                agent::OutcomeV1::Session(self.session.clone())
            }
            agent::CommandV1::Events {
                session,
                after: None,
                limit,
            } if session == self.session.session && limit == 64 => {
                agent::OutcomeV1::Events(agent::EventPageV1 {
                    session,
                    entries: vec![agent::EventRecordV1 {
                        cursor: 0,
                        event: agent::EventV1::Permission(self.permission.clone()),
                    }],
                    next_after: None,
                })
            }
            other => panic!("unexpected agent command {other:?}"),
        };
        Ok(agent::ReplyV1::Accepted(outcome))
    }
}

#[test]
fn cli_views_then_confirms_only_the_server_prepared_permission() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("p");
    fs::create_dir(&root).unwrap();
    let database = root.join("participation.db");
    let participation_socket = root.join("participation.sock");
    let agent_socket = root.join("agent.sock");
    let binding = binding();
    let request = request();
    prime(&database, binding, &request);

    let calls = Arc::new(AtomicUsize::new(0));
    let service = AgentParticipationService::new(
        AgentParticipationSqliteStore::open(&database, [1; 16], limits()).unwrap(),
    );
    let mut participation_daemon = AgentParticipationDaemon::bind_new(
        &participation_socket,
        service,
        ParticipationAuthentication {
            calls: Arc::clone(&calls),
            mutate: false,
        },
        LimitsV1::default(),
        Duration::from_secs(5),
        8,
    )
    .unwrap();
    let participation_worker = thread::spawn(move || {
        participation_daemon.serve_next().unwrap();
        participation_daemon.serve_next().unwrap();
    });

    let session = agent::SessionViewV1 {
        session: agent::SessionIdV1(binding.session.0),
        revision: 2,
        phase: agent::PhaseV1::AwaitingPermission,
        profile_document_digest: agent::DigestV1([30; 32]),
        active_turn: Some(request.turn),
        last_event_cursor: Some(0),
        capabilities: agent::CapabilitiesV1 {
            permissions: true,
            ..agent::CapabilitiesV1::default()
        },
    };
    let mut agent_daemon = AgentDaemon::bind_new(
        &agent_socket,
        AgentService::new(Gateway {
            session,
            permission: request.clone(),
        }),
        ReadAuthentication,
        LimitsV1::default(),
        Duration::from_secs(5),
        4,
    )
    .unwrap();
    let agent_worker = thread::spawn(move || agent_daemon.serve_next().unwrap());

    let binary = env!("CARGO_BIN_EXE_moira-tui");
    let viewed = Command::new(binary)
        .arg("permission-view")
        .arg(&participation_socket)
        .arg(&agent_socket)
        .arg(hex(&binding.participation.0))
        .output()
        .unwrap();
    assert!(
        viewed.status.success(),
        "{}",
        String::from_utf8_lossy(&viewed.stderr)
    );
    let view: Value = serde_json::from_slice(&viewed.stdout).unwrap();
    assert_eq!(view["status"], "current");
    assert_eq!(view["options"][0]["id"], hex(&request.options[0].id.0));
    let digest = view["confirmation_digest"].as_str().unwrap();

    let confirmed = Command::new(binary)
        .arg("permission-confirm")
        .arg(&participation_socket)
        .arg(hex(&binding.participation.0))
        .arg(hex(&request.options[0].id.0))
        .arg(digest)
        .arg("CONFIRM")
        .output()
        .unwrap();
    assert!(
        confirmed.status.success(),
        "{}",
        String::from_utf8_lossy(&confirmed.stderr)
    );
    let confirmation: Value = serde_json::from_slice(&confirmed.stdout).unwrap();
    assert_eq!(confirmation["status"], "accepted");
    assert_eq!(confirmation["pending_operation"], "decide_permission");
    assert_eq!(calls.load(Ordering::Relaxed), 1);

    agent_worker.join().unwrap();
    participation_worker.join().unwrap();
    for suffix in ["", "-journal", ".owner.lock"] {
        let _ = fs::remove_file(format!("{}{}", database.display(), suffix));
    }
    let _ = fs::remove_dir(&root);
}

#[test]
fn cli_views_then_confirms_only_the_server_prepared_cancellation() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("c");
    fs::create_dir(&root).unwrap();
    let database = root.join("participation.db");
    let socket = root.join("participation.sock");
    let binding = binding();
    prime_active(&database, binding, p::AgentTurnIdV1([22; 16]));
    let calls = Arc::new(AtomicUsize::new(0));
    let service = AgentParticipationService::new(
        AgentParticipationSqliteStore::open(&database, [1; 16], limits()).unwrap(),
    );
    let mut daemon = AgentParticipationDaemon::bind_new(
        &socket,
        service,
        ParticipationAuthentication {
            calls: Arc::clone(&calls),
            mutate: true,
        },
        LimitsV1::default(),
        Duration::from_secs(5),
        8,
    )
    .unwrap();
    let worker = thread::spawn(move || {
        daemon.serve_next().unwrap();
        daemon.serve_next().unwrap();
    });
    let binary = env!("CARGO_BIN_EXE_moira-tui");
    let viewed = Command::new(binary)
        .arg("cancel-view")
        .arg(&socket)
        .arg(hex(&binding.participation.0))
        .output()
        .unwrap();
    assert!(
        viewed.status.success(),
        "{}",
        String::from_utf8_lossy(&viewed.stderr)
    );
    let view: Value = serde_json::from_slice(&viewed.stdout).unwrap();
    assert_eq!(view["status"], "current");
    let digest = view["confirmation_digest"].as_str().unwrap();
    let confirmed = Command::new(binary)
        .arg("cancel-confirm")
        .arg(&socket)
        .arg(hex(&binding.participation.0))
        .arg(digest)
        .arg("CONFIRM")
        .output()
        .unwrap();
    assert!(
        confirmed.status.success(),
        "{}",
        String::from_utf8_lossy(&confirmed.stderr)
    );
    let confirmation: Value = serde_json::from_slice(&confirmed.stdout).unwrap();
    assert_eq!(confirmation["status"], "accepted");
    assert_eq!(confirmation["pending_operation"], "cancel");
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    worker.join().unwrap();
    cleanup(&database, &root);
}

fn prime(database: &Path, binding: p::BindingV1, request: &agent::PermissionRequestV1) {
    let mut service = AgentParticipationService::new(
        AgentParticipationSqliteStore::open(database, [1; 16], limits()).unwrap(),
    );
    apply(&mut service, None, p::MutationV1::Admit(binding));
    let open = p::OperationV1::Open {
        key: p::IdempotencyKeyV1([20; 16]),
        execution: p::ExecutionIdV1([21; 16]),
    };
    apply(
        &mut service,
        Some(0),
        p::MutationV1::Prepare {
            participation: binding.participation,
            expected_revision: 0,
            operation: open,
        },
    );
    apply(
        &mut service,
        Some(1),
        p::MutationV1::Observe {
            participation: binding.participation,
            expected_revision: 1,
            key: p::IdempotencyKeyV1([20; 16]),
            observation: observation(p::PhaseV1::Ready, 1, None, None),
        },
    );
    let prompt = p::OperationV1::Prompt {
        key: p::IdempotencyKeyV1([23; 16]),
        turn: p::AgentTurnIdV1(request.turn.0),
        execution: p::ExecutionIdV1([24; 16]),
        prompt_digest: [25; 32],
        attempt: binding.attempt,
    };
    apply(
        &mut service,
        Some(2),
        p::MutationV1::Prepare {
            participation: binding.participation,
            expected_revision: 2,
            operation: prompt,
        },
    );
    let mut option_ids = [p::PermissionOptionIdV1([0; 16]); p::MAX_PERMISSION_OPTIONS_V1];
    for (target, source) in option_ids.iter_mut().zip(&request.options) {
        *target = p::PermissionOptionIdV1(source.id.0);
    }
    let permission = p::PermissionEvidenceV1 {
        request: p::PermissionRequestIdV1(request.request.0),
        turn: p::AgentTurnIdV1(request.turn.0),
        capability_digest: Sha256::digest(&request.capability).into(),
        options_digest: options_digest(&request.options),
        option_count: request.options.len().try_into().unwrap(),
        option_ids,
    };
    apply(
        &mut service,
        Some(3),
        p::MutationV1::Observe {
            participation: binding.participation,
            expected_revision: 3,
            key: p::IdempotencyKeyV1([23; 16]),
            observation: observation(
                p::PhaseV1::AwaitingPermission,
                2,
                Some(permission.turn),
                Some(permission),
            ),
        },
    );
}

fn prime_active(database: &Path, binding: p::BindingV1, turn: p::AgentTurnIdV1) {
    let mut service = AgentParticipationService::new(
        AgentParticipationSqliteStore::open(database, [1; 16], limits()).unwrap(),
    );
    apply(&mut service, None, p::MutationV1::Admit(binding));
    apply(
        &mut service,
        Some(0),
        p::MutationV1::Prepare {
            participation: binding.participation,
            expected_revision: 0,
            operation: p::OperationV1::Open {
                key: p::IdempotencyKeyV1([20; 16]),
                execution: p::ExecutionIdV1([21; 16]),
            },
        },
    );
    apply(
        &mut service,
        Some(1),
        p::MutationV1::Observe {
            participation: binding.participation,
            expected_revision: 1,
            key: p::IdempotencyKeyV1([20; 16]),
            observation: observation(p::PhaseV1::Ready, 1, None, None),
        },
    );
    apply(
        &mut service,
        Some(2),
        p::MutationV1::Prepare {
            participation: binding.participation,
            expected_revision: 2,
            operation: p::OperationV1::Prompt {
                key: p::IdempotencyKeyV1([23; 16]),
                turn,
                execution: p::ExecutionIdV1([24; 16]),
                prompt_digest: [25; 32],
                attempt: binding.attempt,
            },
        },
    );
    apply(
        &mut service,
        Some(3),
        p::MutationV1::Observe {
            participation: binding.participation,
            expected_revision: 3,
            key: p::IdempotencyKeyV1([23; 16]),
            observation: observation(p::PhaseV1::TurnActive, 2, Some(turn), None),
        },
    );
}

fn cleanup(database: &Path, root: &Path) {
    for suffix in ["", "-journal", ".owner.lock"] {
        let _ = fs::remove_file(format!("{}{}", database.display(), suffix));
    }
    let _ = fs::remove_dir(root);
}

fn apply(
    service: &mut AgentParticipationService<AgentParticipationSqliteStore>,
    expected: Option<u64>,
    mutation: p::MutationV1,
) {
    assert!(matches!(
        service
            .execute(p::CommandV1::Apply {
                expected_storage_revision: expected,
                mutation: Box::new(mutation),
            })
            .unwrap(),
        p::ReplyV1::Accepted(_)
    ));
}

fn observation(
    phase: p::PhaseV1,
    session_revision: u64,
    active_turn: Option<p::AgentTurnIdV1>,
    permission: Option<p::PermissionEvidenceV1>,
) -> p::ObservationV1 {
    p::ObservationV1 {
        phase,
        session_revision,
        active_turn,
        last_event_cursor: permission.map(|_| 0),
        profile_document_digest: [30; 32],
        capabilities: p::CapabilitiesV1 {
            permissions: true,
            ..p::CapabilitiesV1::default()
        },
        permission,
        terminal: None,
    }
}

fn request() -> agent::PermissionRequestV1 {
    agent::PermissionRequestV1 {
        request: agent::PermissionRequestIdV1([26; 16]),
        turn: agent::TurnIdV1([22; 16]),
        capability: b"terminal: cargo test".to_vec(),
        description: b"run the bounded test suite".to_vec(),
        options: vec![
            agent::PermissionOptionV1 {
                id: agent::PermissionOptionIdV1([32; 16]),
                kind: agent::PermissionOptionKindV1::AllowOnce,
                label: b"allow once".to_vec(),
            },
            agent::PermissionOptionV1 {
                id: agent::PermissionOptionIdV1([33; 16]),
                kind: agent::PermissionOptionKindV1::RejectOnce,
                label: b"reject".to_vec(),
            },
        ],
    }
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

fn binding() -> p::BindingV1 {
    p::BindingV1 {
        participation: p::ParticipationIdV1([2; 16]),
        workflow: WorkflowIdV1([3; 16]),
        deliberation: d::DeliberationIdV1([4; 16]),
        member: d::EnsembleMemberIdV1([5; 16]),
        session: p::AgentSessionIdV1([6; 16]),
        profile: p::ProfileIdV1([7; 16]),
        attempt: p::AttemptV1 {
            task_key: [8; 16],
            incarnation: 1,
            attempt: 1,
        },
        method: d::MethodRefV1 {
            machine_digest: [9; 32],
            state_digest: [10; 32],
        },
        grounding: d::GroundingRefV1 {
            authority_digest: [11; 32],
            surface_digest: [12; 32],
            depth: d::SurfaceDepthV1::Activated,
        },
        persona: d::PersonaLensV1::Researcher,
    }
}

fn limits() -> AgentParticipationStoreLimitsV1 {
    AgentParticipationStoreLimitsV1 {
        codec: CodecLimitsV1 {
            max_bytes: 65_536,
            domain: DomainLimits {
                max_participations: 4,
            },
        },
        max_database_pages: 64,
    }
}

fn path(kind: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "mtp-{kind}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

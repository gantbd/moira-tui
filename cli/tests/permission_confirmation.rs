#![cfg(unix)]

use std::{
    convert::Infallible,
    fs,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
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
use moira_host::{AgentGateway, AgentPromptStoreV1};
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
use moira_sqlite::agent_prompt::{AgentPromptSqliteStore, AgentPromptStoreLimitsV1};
use moira_wire::{
    LimitsV1, ReadyV1, decode_agent_participation_request, decode_agent_request,
    encode_agent_participation_reply, encode_agent_reply, encode_ready,
};
use ratatui::{Terminal, backend::TestBackend};
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
        last_context: None,
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

#[test]
fn cli_views_then_confirms_only_the_server_prepared_reattach() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("rt");
    fs::create_dir(&root).unwrap();
    let database = root.join("participation.db");
    let socket = root.join("participation.sock");
    let binding = binding();
    prime_reattach(&database, binding);
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
        daemon.serve_next().unwrap();
    });
    let binary = env!("CARGO_BIN_EXE_moira-tui");
    let viewed = Command::new(binary)
        .arg("reattach-view")
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
    assert!(view.get("attempt").is_none());
    let digest = view["confirmation_digest"].as_str().unwrap();
    let confirmed = Command::new(binary)
        .arg("reattach-confirm")
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
    assert_eq!(confirmation["pending_operation"], "reattach");
    let replay = Command::new(binary)
        .arg("reattach-confirm")
        .arg(&socket)
        .arg(hex(&binding.participation.0))
        .arg(digest)
        .arg("CONFIRM")
        .output()
        .unwrap();
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let replay: Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    worker.join().unwrap();
    cleanup(&database, &root);
}

#[test]
fn cli_submits_bounded_prompt_bytes_without_operation_identities() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("pr");
    fs::create_dir(&root).unwrap();
    let database = root.join("participation.db");
    let prompt_database = root.join("prompts.db");
    let prompt_file = root.join("prompt.txt");
    let socket = root.join("participation.sock");
    let binding = binding();
    prime_ready(&database, binding);
    fs::write(&prompt_file, b"answer from the retained theorem").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let service = AgentParticipationService::new(
        AgentParticipationSqliteStore::open(&database, [1; 16], limits()).unwrap(),
    );
    let prompt_store = AgentPromptSqliteStore::open(
        &prompt_database,
        [2; 16],
        AgentPromptStoreLimitsV1 {
            max_entries: 4,
            max_prompt_bytes: 128,
            max_database_pages: 64,
        },
    )
    .unwrap();
    let mut daemon = AgentParticipationDaemon::bind_new_with_prompt_store(
        &socket,
        service,
        ParticipationAuthentication {
            calls: Arc::clone(&calls),
            mutate: true,
        },
        prompt_store,
        LimitsV1::default(),
        Duration::from_secs(5),
        2,
    )
    .unwrap();
    let worker = thread::spawn(move || {
        daemon.serve_next().unwrap();
        daemon
    });
    let submitted = Command::new(env!("CARGO_BIN_EXE_moira-tui"))
        .arg("prompt-submit")
        .arg(&socket)
        .arg(hex(&binding.participation.0))
        .arg(&prompt_file)
        .arg("CONFIRM")
        .output()
        .unwrap();
    assert!(
        submitted.status.success(),
        "{}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    let outcome: Value = serde_json::from_slice(&submitted.stdout).unwrap();
    assert_eq!(outcome["status"], "accepted");
    assert_eq!(outcome["pending_operation"], "prompt");
    assert_eq!(outcome["replayed"], false);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    let daemon = worker.join().unwrap();
    let (_, _, mut prompt_store) = daemon.into_prompt_parts();
    let digest: [u8; 32] = Sha256::digest(b"answer from the retained theorem").into();
    assert_eq!(
        prompt_store.get(digest).unwrap().unwrap(),
        b"answer from the retained theorem"
    );
    drop(prompt_store);
    for value in [&database, &prompt_database] {
        for suffix in ["", "-journal", ".owner.lock"] {
            let _ = fs::remove_file(format!("{}{}", value.display(), suffix));
        }
    }
    let _ = fs::remove_file(prompt_file);
    let _ = fs::remove_dir(root);
}

#[test]
fn cli_reports_unknown_after_durable_prompt_commit_and_exact_replay_is_safe() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("plr");
    fs::create_dir(&root).unwrap();
    let database = root.join("participation.db");
    let prompt_database = root.join("prompts.db");
    let prompt_file = root.join("prompt.txt");
    let socket = root.join("participation.sock");
    let binding = binding();
    let prompt = b"one durable prompt across a lost reply".to_vec();
    prime_ready(&database, binding);
    fs::write(&prompt_file, &prompt).unwrap();

    let listener = UnixListener::bind(&socket).unwrap();
    let mut service = AgentParticipationService::new(
        AgentParticipationSqliteStore::open(&database, [1; 16], limits()).unwrap(),
    );
    let mut prompts = AgentPromptSqliteStore::open(
        &prompt_database,
        [2; 16],
        AgentPromptStoreLimitsV1 {
            max_entries: 4,
            max_prompt_bytes: 128,
            max_database_pages: 64,
        },
    )
    .unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let ready = encode_ready(
            ReadyV1 {
                protocol_version: 1,
                max_frame_bytes: 64 * 1024,
                max_requests: 1,
            },
            LimitsV1::default(),
        )
        .unwrap();
        write_wire_frame(&mut stream, &ready);
        let request =
            decode_agent_participation_request(&read_wire_frame(&mut stream), LimitsV1::default())
                .unwrap();
        let p::CommandV1::SubmitPrompt(submission) = request else {
            panic!("unexpected prompt command {request:?}");
        };
        assert_eq!(submission.participation, binding.participation);
        assert_eq!(submission.prompt, prompt);
        let committed = service
            .execute_prompt_authorized(
                AccessV1 {
                    read: true,
                    mutate: true,
                },
                submission,
                &mut prompts,
            )
            .unwrap();
        assert!(matches!(
            committed,
            p::ReplyV1::Accepted(value)
                if matches!(*value, p::OutcomeV1::Applied { replayed: false, .. })
        ));
        drop(stream);
        (service, prompts)
    });

    let submitted = Command::new(env!("CARGO_BIN_EXE_moira-tui"))
        .arg("prompt-submit")
        .arg(&socket)
        .arg(hex(&binding.participation.0))
        .arg(&prompt_file)
        .arg("CONFIRM")
        .output()
        .unwrap();
    assert!(!submitted.status.success());
    let outcome: Value = serde_json::from_slice(&submitted.stdout).unwrap();
    assert_eq!(outcome["status"], "outcome_unknown");
    assert_eq!(outcome["retry_safe"], false);
    assert_eq!(
        outcome["recovery"],
        "inspect participation before any further action"
    );

    let (mut service, mut prompts) = worker.join().unwrap();
    let replay = service
        .execute_prompt_authorized(
            AccessV1 {
                read: true,
                mutate: true,
            },
            p::PromptSubmissionV1 {
                participation: binding.participation,
                prompt: b"one durable prompt across a lost reply".to_vec(),
            },
            &mut prompts,
        )
        .unwrap();
    assert!(matches!(
        replay,
        p::ReplyV1::Accepted(value)
            if matches!(*value, p::OutcomeV1::Applied { replayed: true, .. })
    ));
    let digest: [u8; 32] = Sha256::digest(b"one durable prompt across a lost reply").into();
    assert_eq!(
        prompts.get(digest).unwrap().unwrap(),
        b"one durable prompt across a lost reply"
    );

    drop(service);
    drop(prompts);
    for value in [&database, &prompt_database] {
        for suffix in ["", "-journal", ".owner.lock"] {
            let _ = fs::remove_file(format!("{}{}", value.display(), suffix));
        }
    }
    let _ = fs::remove_file(prompt_file);
    let _ = fs::remove_file(socket);
    let _ = fs::remove_dir(root);
}

#[test]
fn reusable_participation_component_reads_the_real_bounded_daemon() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("pc");
    fs::create_dir(&root).unwrap();
    let database = root.join("participation.db");
    let socket = root.join("participation.sock");
    let binding = binding();
    prime_ready(&database, binding);
    let service = AgentParticipationService::new(
        AgentParticipationSqliteStore::open(&database, [1; 16], limits()).unwrap(),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let mut daemon = AgentParticipationDaemon::bind_new(
        &socket,
        service,
        ParticipationAuthentication {
            calls: Arc::clone(&calls),
            mutate: false,
        },
        LimitsV1::default(),
        Duration::from_secs(5),
        4,
    )
    .unwrap();
    let worker = thread::spawn(move || daemon.serve_next().unwrap());
    let port = moira_tui_local_client::LocalAgentParticipationPort::connect(
        &socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let mut component = moira_tui::ParticipationComponent::new(port, binding.participation);
    for _ in 0..100 {
        component.poll();
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal
            .draw(|frame| component.draw_in(frame, frame.area(), true))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        if rendered.contains("phase: Ready") {
            assert!(rendered.contains("participation revision: 2"));
            assert!(rendered.contains("persona: Researcher"));
            assert!(rendered.contains("grounding depth: Activated"));
            drop(component);
            worker.join().unwrap();
            assert_eq!(calls.load(Ordering::Relaxed), 0);
            cleanup(&database, &root);
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("real participation daemon did not render through the component");
}

#[test]
fn reusable_agent_component_reads_one_stable_real_daemon_connection() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("ac");
    fs::create_dir(&root).unwrap();
    let socket = root.join("agent.sock");
    let request = request();
    let session = agent::SessionViewV1 {
        session: agent::SessionIdV1([6; 16]),
        revision: 2,
        phase: agent::PhaseV1::AwaitingPermission,
        profile_document_digest: agent::DigestV1([30; 32]),
        active_turn: Some(request.turn),
        last_event_cursor: Some(0),
        capabilities: agent::CapabilitiesV1 {
            permissions: true,
            ..agent::CapabilitiesV1::default()
        },
        last_context: None,
    };
    let mut daemon = AgentDaemon::bind_new(
        &socket,
        AgentService::new(Gateway {
            session: session.clone(),
            permission: request,
        }),
        ReadAuthentication,
        LimitsV1::default(),
        Duration::from_secs(5),
        4,
    )
    .unwrap();
    let worker = thread::spawn(move || daemon.serve_next().unwrap());
    let port = moira_tui_local_client::LocalAgentPort::connect(
        &socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let mut component = moira_tui::AgentComponent::new(port, session.session);
    wait_agent(&mut component, "Stable agent revision 2");
    let rendered = render_agent(&component);
    assert!(rendered.contains("phase: AwaitingPermission"));
    assert!(rendered.contains("permission[2 option(s)]"));
    drop(component);
    worker.join().unwrap();
    let _ = fs::remove_file(socket);
    let _ = fs::remove_dir(root);
}

#[test]
fn agent_inspect_cli_preserves_the_complete_public_capability_projection() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("ai");
    fs::create_dir(&root).unwrap();
    let socket = root.join("a.sock");
    let request = request();
    let session = agent::SessionViewV1 {
        session: agent::SessionIdV1([8; 16]),
        revision: 4,
        phase: agent::PhaseV1::ReattachRequired,
        profile_document_digest: agent::DigestV1([32; 32]),
        active_turn: None,
        last_event_cursor: None,
        capabilities: agent::CapabilitiesV1 {
            load_session: true,
            resume_session: true,
            permissions: true,
            filesystem_read: true,
            filesystem_write: true,
            terminal: true,
            session_modes: true,
            session_models: true,
        },
        last_context: None,
    };
    let mut daemon = AgentDaemon::bind_new(
        &socket,
        AgentService::new(Gateway {
            session: session.clone(),
            permission: request,
        }),
        ReadAuthentication,
        LimitsV1::default(),
        Duration::from_secs(5),
        1,
    )
    .unwrap();
    let worker = thread::spawn(move || daemon.serve_next().unwrap());

    let output = Command::new(env!("CARGO_BIN_EXE_moira-tui"))
        .arg("agent-inspect")
        .arg(&socket)
        .arg(hex(&session.session.0))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let observation: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(observation["status"], "current");
    assert_eq!(observation["phase"], "ReattachRequired");
    assert_eq!(
        observation["recovery"],
        "explicit_governed_reattach_required"
    );
    assert_eq!(
        observation["capabilities"],
        serde_json::json!({
            "load_session": true,
            "resume_session": true,
            "permissions": true,
            "filesystem_read": true,
            "filesystem_write": true,
            "terminal": true,
            "session_modes": true,
            "session_models": true,
        })
    );

    worker.join().unwrap();
    let _ = fs::remove_file(socket);
    let _ = fs::remove_dir(root);
}

#[test]
fn reusable_agent_component_reconnects_after_agent_daemon_rebind() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("ar");
    fs::create_dir(&root).unwrap();
    let socket = root.join("a.sock");
    let request = request();
    let session = agent::SessionViewV1 {
        session: agent::SessionIdV1([7; 16]),
        revision: 3,
        phase: agent::PhaseV1::AwaitingPermission,
        profile_document_digest: agent::DigestV1([31; 32]),
        active_turn: Some(request.turn),
        last_event_cursor: Some(0),
        capabilities: agent::CapabilitiesV1 {
            permissions: true,
            ..agent::CapabilitiesV1::default()
        },
        last_context: None,
    };
    let daemon = AgentDaemon::bind_new(
        &socket,
        AgentService::new(Gateway {
            session: session.clone(),
            permission: request,
        }),
        ReadAuthentication,
        LimitsV1::default(),
        Duration::from_secs(5),
        3,
    )
    .unwrap();
    let first_worker = thread::spawn(move || {
        let mut daemon = daemon;
        daemon.serve_next().unwrap();
        daemon
    });
    let port = moira_tui_local_client::LocalAgentPort::connect(
        &socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let mut component = moira_tui::AgentComponent::new(port, session.session);
    wait_agent(&mut component, "Stable agent revision 3");
    let before = render_agent(&component);
    assert!(before.contains("permission[2 option(s)]"));

    let first_daemon = first_worker.join().unwrap();
    let (service, authentication) = first_daemon.into_parts();
    let gateway = service.into_gateway();
    let daemon = AgentDaemon::bind_new(
        &socket,
        AgentService::new(gateway),
        authentication,
        LimitsV1::default(),
        Duration::from_secs(5),
        3,
    )
    .unwrap();
    let second_worker = thread::spawn(move || {
        let mut daemon = daemon;
        daemon.serve_next().unwrap();
    });

    component.update(moira_tui::Input::Refresh, true);
    wait_agent(&mut component, "Stable agent revision 3");
    assert_eq!(render_agent(&component), before);

    drop(component);
    second_worker.join().unwrap();
    let _ = fs::remove_file(socket);
    let _ = fs::remove_dir(root);
}

#[test]
fn interactive_cancellation_component_uses_one_real_daemon_connection() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("cc");
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
    let worker = thread::spawn(move || daemon.serve_next().unwrap());
    let port = moira_tui_local_client::LocalAgentParticipationPort::connect(
        &socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let mut component = moira_tui::CancellationControlComponent::new(port, binding.participation);
    wait_cancel(&mut component, "server challenge:");
    component.update(moira_tui::CancelControlInput::Arm, true);
    component.update(moira_tui::CancelControlInput::Confirm, true);
    wait_cancel(&mut component, "Cancellation durably accepted");
    drop(component);
    worker.join().unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    cleanup(&database, &root);
}

#[test]
fn interactive_prompt_component_uses_one_real_daemon_connection() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("pmc");
    fs::create_dir(&root).unwrap();
    let database = root.join("participation.db");
    let prompt_database = root.join("prompts.db");
    let socket = root.join("participation.sock");
    let binding = binding();
    prime_ready(&database, binding);
    let service = AgentParticipationService::new(
        AgentParticipationSqliteStore::open(&database, [1; 16], limits()).unwrap(),
    );
    let prompt_store = AgentPromptSqliteStore::open(
        &prompt_database,
        [2; 16],
        AgentPromptStoreLimitsV1 {
            max_entries: 4,
            max_prompt_bytes: 128,
            max_database_pages: 64,
        },
    )
    .unwrap();
    let mut daemon = AgentParticipationDaemon::bind_new_with_prompt_store(
        &socket,
        service,
        ParticipationAuthentication {
            calls: Arc::new(AtomicUsize::new(0)),
            mutate: true,
        },
        prompt_store,
        LimitsV1::default(),
        Duration::from_secs(5),
        8,
    )
    .unwrap();
    let worker = thread::spawn(move || {
        daemon.serve_next().unwrap();
        daemon
    });
    let port = moira_tui_local_client::LocalAgentParticipationPort::connect(
        &socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let prompt = b"real daemon exact prompt".to_vec();
    let mut component =
        moira_tui::PromptControlComponent::try_new(port, binding.participation, prompt.clone())
            .unwrap();
    wait_prompt(&mut component, "Stable governed participation retained");
    component.update(moira_tui::PromptControlInput::Arm, true);
    component.update(moira_tui::PromptControlInput::Confirm, true);
    wait_prompt(&mut component, "Prompt durably accepted");
    drop(component);
    let daemon = worker.join().unwrap();
    let (_, _, mut prompts) = daemon.into_prompt_parts();
    let digest: [u8; 32] = Sha256::digest(&prompt).into();
    assert_eq!(prompts.get(digest).unwrap(), Some(prompt));
    drop(prompts);
    for value in [&database, &prompt_database] {
        for suffix in ["", "-journal", ".owner.lock"] {
            let _ = fs::remove_file(format!("{}{}", value.display(), suffix));
        }
    }
    let _ = fs::remove_dir(root);
}

#[test]
fn interactive_prompt_recovers_durable_state_after_a_lost_reply_without_resubmission() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("pcr");
    fs::create_dir(&root).unwrap();
    let database = root.join("participation.db");
    let prompt_database = root.join("prompts.db");
    let socket = root.join("participation.sock");
    let binding = binding();
    let prompt = b"one component prompt across uncertainty".to_vec();
    prime_ready(&database, binding);

    let listener = UnixListener::bind(&socket).unwrap();
    let mut service = AgentParticipationService::new(
        AgentParticipationSqliteStore::open(&database, [1; 16], limits()).unwrap(),
    );
    let mut prompts = AgentPromptSqliteStore::open(
        &prompt_database,
        [2; 16],
        AgentPromptStoreLimitsV1 {
            max_entries: 4,
            max_prompt_bytes: 128,
            max_database_pages: 64,
        },
    )
    .unwrap();
    let expected_prompt = prompt.clone();
    let worker = thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        write_ready(&mut first, 3);
        for _ in 0..2 {
            let command = decode_agent_participation_request(
                &read_wire_frame(&mut first),
                LimitsV1::default(),
            )
            .unwrap();
            assert_eq!(
                command,
                p::CommandV1::Participation {
                    participation: binding.participation,
                }
            );
            let reply = service
                .execute_authorized(
                    AccessV1 {
                        read: true,
                        mutate: false,
                    },
                    command,
                )
                .unwrap();
            write_participation_reply(&mut first, &reply);
        }
        let command =
            decode_agent_participation_request(&read_wire_frame(&mut first), LimitsV1::default())
                .unwrap();
        let p::CommandV1::SubmitPrompt(submission) = command else {
            panic!("unexpected prompt command {command:?}");
        };
        assert_eq!(submission.participation, binding.participation);
        assert_eq!(submission.prompt, expected_prompt);
        let committed = service
            .execute_prompt_authorized(
                AccessV1 {
                    read: true,
                    mutate: true,
                },
                submission,
                &mut prompts,
            )
            .unwrap();
        assert!(matches!(
            committed,
            p::ReplyV1::Accepted(value)
                if matches!(*value, p::OutcomeV1::Applied { replayed: false, .. })
        ));
        drop(first);

        let (mut second, _) = listener.accept().unwrap();
        write_ready(&mut second, 2);
        for _ in 0..2 {
            let command = decode_agent_participation_request(
                &read_wire_frame(&mut second),
                LimitsV1::default(),
            )
            .unwrap();
            assert_eq!(
                command,
                p::CommandV1::Participation {
                    participation: binding.participation,
                }
            );
            let reply = service
                .execute_authorized(
                    AccessV1 {
                        read: true,
                        mutate: false,
                    },
                    command,
                )
                .unwrap();
            write_participation_reply(&mut second, &reply);
        }
        (service, prompts)
    });

    let port = moira_tui_local_client::LocalAgentParticipationPort::connect(
        &socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let mut component =
        moira_tui::PromptControlComponent::try_new(port, binding.participation, prompt.clone())
            .unwrap();
    wait_prompt(&mut component, "Stable governed participation retained");
    component.update(moira_tui::PromptControlInput::Arm, true);
    component.update(moira_tui::PromptControlInput::Confirm, true);
    wait_prompt(&mut component, "OUTCOME UNKNOWN");

    component.update(moira_tui::PromptControlInput::Arm, true);
    component.update(moira_tui::PromptControlInput::Confirm, true);
    thread::sleep(Duration::from_millis(5));
    component.poll();
    assert!(render_prompt(&component).contains("OUTCOME UNKNOWN"));

    component.update(moira_tui::PromptControlInput::Refresh, true);
    wait_prompt(&mut component, "Stable governed participation retained");
    let rendered = render_prompt(&component);
    assert!(rendered.contains("pending operation: true"));
    assert!(!rendered.contains("OUTCOME UNKNOWN"));
    drop(component);

    let (service, mut prompts) = worker.join().unwrap();
    let digest: [u8; 32] = Sha256::digest(&prompt).into();
    assert_eq!(prompts.get(digest).unwrap(), Some(prompt));
    drop(service);
    drop(prompts);
    for value in [&database, &prompt_database] {
        for suffix in ["", "-journal", ".owner.lock"] {
            let _ = fs::remove_file(format!("{}{}", value.display(), suffix));
        }
    }
    let _ = fs::remove_file(socket);
    let _ = fs::remove_dir(root);
}

#[test]
fn interactive_cancellation_recovers_durable_state_after_a_lost_reply_without_resubmission() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("ccr");
    fs::create_dir(&root).unwrap();
    let database = root.join("participation.db");
    let socket = root.join("participation.sock");
    let binding = binding();
    let turn = p::AgentTurnIdV1([22; 16]);
    prime_active(&database, binding, turn);

    let listener = UnixListener::bind(&socket).unwrap();
    let mut service = AgentParticipationService::new(
        AgentParticipationSqliteStore::open(&database, [1; 16], limits()).unwrap(),
    );
    let worker = thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        write_ready(&mut first, 5);
        for index in 0..4 {
            let command = decode_agent_participation_request(
                &read_wire_frame(&mut first),
                LimitsV1::default(),
            )
            .unwrap();
            let expected = if index % 2 == 0 {
                p::CommandV1::Participation {
                    participation: binding.participation,
                }
            } else {
                p::CommandV1::CancelConfirmation {
                    participation: binding.participation,
                }
            };
            assert_eq!(command, expected);
            let reply = service
                .execute_authorized(
                    AccessV1 {
                        read: true,
                        mutate: false,
                    },
                    command,
                )
                .unwrap();
            write_participation_reply(&mut first, &reply);
        }
        let command =
            decode_agent_participation_request(&read_wire_frame(&mut first), LimitsV1::default())
                .unwrap();
        let p::CommandV1::ConfirmCancel(confirmation) = command else {
            panic!("unexpected cancellation command {command:?}");
        };
        assert_eq!(confirmation.participation, binding.participation);
        let committed = service
            .execute_authorized(
                AccessV1 {
                    read: true,
                    mutate: true,
                },
                p::CommandV1::ConfirmCancel(confirmation),
            )
            .unwrap();
        assert!(matches!(
            committed,
            p::ReplyV1::Accepted(value)
                if matches!(*value, p::OutcomeV1::Applied { replayed: false, .. })
        ));
        drop(first);

        let (mut second, _) = listener.accept().unwrap();
        write_ready(&mut second, 2);
        for expected in [
            p::CommandV1::Participation {
                participation: binding.participation,
            },
            p::CommandV1::CancelConfirmation {
                participation: binding.participation,
            },
        ] {
            let command = decode_agent_participation_request(
                &read_wire_frame(&mut second),
                LimitsV1::default(),
            )
            .unwrap();
            assert_eq!(command, expected);
            let reply = service
                .execute_authorized(
                    AccessV1 {
                        read: true,
                        mutate: false,
                    },
                    command,
                )
                .unwrap();
            write_participation_reply(&mut second, &reply);
        }
        service
    });

    let port = moira_tui_local_client::LocalAgentParticipationPort::connect(
        &socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let mut component = moira_tui::CancellationControlComponent::new(port, binding.participation);
    wait_cancel(&mut component, "server challenge:");
    component.update(moira_tui::CancelControlInput::Arm, true);
    component.update(moira_tui::CancelControlInput::Confirm, true);
    wait_cancel(&mut component, "OUTCOME UNKNOWN");

    component.update(moira_tui::CancelControlInput::Arm, true);
    component.update(moira_tui::CancelControlInput::Confirm, true);
    thread::sleep(Duration::from_millis(5));
    component.poll();
    assert!(render_cancel(&component).contains("OUTCOME UNKNOWN"));

    component.update(moira_tui::CancelControlInput::Refresh, true);
    wait_cancel(&mut component, "No cancellable active turn");
    let rendered = render_cancel(&component);
    assert!(rendered.contains("no cancellable active turn"));
    assert!(!rendered.contains("OUTCOME UNKNOWN"));
    drop(component);

    let mut service = worker.join().unwrap();
    let current = service
        .execute(p::CommandV1::Participation {
            participation: binding.participation,
        })
        .unwrap();
    assert!(matches!(
        current,
        p::ReplyV1::Accepted(value)
            if matches!(
                *value,
                p::OutcomeV1::Participation(Some(p::ParticipationViewV1 {
                    pending: Some(p::OperationV1::Cancel { turn: retained, .. }),
                    ..
                })) if retained == turn
            )
    ));
    drop(service);
    cleanup(&database, &root);
}

#[test]
fn interactive_permission_recovers_authorized_state_after_a_lost_reply_without_resubmission() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("ppr");
    fs::create_dir(&root).unwrap();
    let database = root.join("participation.db");
    let participation_socket = root.join("participation.sock");
    let agent_socket = root.join("agent.sock");
    let binding = binding();
    let request = request();
    prime(&database, binding, &request);
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
        last_context: None,
    };

    let participation_listener = UnixListener::bind(&participation_socket).unwrap();
    let agent_listener = UnixListener::bind(&agent_socket).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let authority_calls = Arc::clone(&calls);
    let permission_request_id = request.request.0;
    let selected_option_id = request.options[0].id.0;
    let agent_request = request.clone();
    let mut service = AgentParticipationService::new(
        AgentParticipationSqliteStore::open(&database, [1; 16], limits()).unwrap(),
    );
    let participation_worker = thread::spawn(move || {
        let mut authorizer = ParticipationAuthentication {
            calls: authority_calls,
            mutate: true,
        };
        let (mut first, _) = participation_listener.accept().unwrap();
        write_ready(&mut first, 5);
        for index in 0..4 {
            let command = decode_agent_participation_request(
                &read_wire_frame(&mut first),
                LimitsV1::default(),
            )
            .unwrap();
            let expected = if index % 2 == 0 {
                p::CommandV1::Participation {
                    participation: binding.participation,
                }
            } else {
                p::CommandV1::PermissionConfirmation {
                    participation: binding.participation,
                }
            };
            assert_eq!(command, expected);
            let reply = service
                .execute_authorized(
                    AccessV1 {
                        read: true,
                        mutate: false,
                    },
                    command,
                )
                .unwrap();
            write_participation_reply(&mut first, &reply);
        }
        let command =
            decode_agent_participation_request(&read_wire_frame(&mut first), LimitsV1::default())
                .unwrap();
        let p::CommandV1::ConfirmPermission(confirmed) = command else {
            panic!("unexpected permission command {command:?}");
        };
        assert_eq!(confirmed.participation, binding.participation);
        assert_eq!(confirmed.option.0, selected_option_id);
        let selection = service.prepare_confirmed_permission(confirmed).unwrap();
        let access = authorizer.authorize_permission(selection).unwrap().unwrap();
        let committed = service
            .execute_permission_authorized(access, p::CommandV1::SelectPermission(selection))
            .unwrap();
        assert!(matches!(
            committed,
            p::ReplyV1::Accepted(value)
                if matches!(*value, p::OutcomeV1::Applied { replayed: false, .. })
        ));
        drop(first);

        let (mut second, _) = participation_listener.accept().unwrap();
        write_ready(&mut second, 2);
        for expected in [
            p::CommandV1::Participation {
                participation: binding.participation,
            },
            p::CommandV1::PermissionConfirmation {
                participation: binding.participation,
            },
        ] {
            let command = decode_agent_participation_request(
                &read_wire_frame(&mut second),
                LimitsV1::default(),
            )
            .unwrap();
            assert_eq!(command, expected);
            let reply = service
                .execute_authorized(
                    AccessV1 {
                        read: true,
                        mutate: false,
                    },
                    command,
                )
                .unwrap();
            write_participation_reply(&mut second, &reply);
        }
        service
    });

    let agent_worker = thread::spawn(move || {
        let (mut stream, _) = agent_listener.accept().unwrap();
        write_ready(&mut stream, 3);
        let mut gateway = Gateway {
            session,
            permission: agent_request,
        };
        for _ in 0..3 {
            let command =
                decode_agent_request(&read_wire_frame(&mut stream), LimitsV1::default()).unwrap();
            let reply = gateway.execute_agent(command).unwrap();
            let bytes = encode_agent_reply(&reply, LimitsV1::default()).unwrap();
            write_wire_frame(&mut stream, &bytes);
        }
    });

    let participation_port = moira_tui_local_client::LocalAgentParticipationPort::connect(
        &participation_socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let agent_port = moira_tui_local_client::LocalAgentPort::connect(
        &agent_socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let mut component = moira_tui::PermissionControlComponent::new(
        participation_port,
        agent_port,
        binding.participation,
    );
    wait_permission(&mut component, "server challenge:");
    component.update(moira_tui::PermissionControlInput::Arm, true);
    component.update(moira_tui::PermissionControlInput::Confirm, true);
    wait_permission(&mut component, "OUTCOME UNKNOWN");

    component.update(moira_tui::PermissionControlInput::Arm, true);
    component.update(moira_tui::PermissionControlInput::Confirm, true);
    thread::sleep(Duration::from_millis(5));
    component.poll();
    assert!(render_permission(&component).contains("OUTCOME UNKNOWN"));

    component.update(moira_tui::PermissionControlInput::Refresh, true);
    wait_permission(&mut component, "Participation is not awaiting permission");
    let rendered = render_permission(&component);
    assert!(rendered.contains("no presentable permission request"));
    assert!(!rendered.contains("OUTCOME UNKNOWN"));
    drop(component);

    agent_worker.join().unwrap();
    let mut service = participation_worker.join().unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    let current = service
        .execute(p::CommandV1::Participation {
            participation: binding.participation,
        })
        .unwrap();
    assert!(matches!(
        current,
        p::ReplyV1::Accepted(value)
            if matches!(
                *value,
                p::OutcomeV1::Participation(Some(p::ParticipationViewV1 {
                    pending: Some(p::OperationV1::DecidePermission {
                        request: retained_request,
                        option,
                        authority,
                        ..
                    }),
                    ..
                })) if retained_request.0 == permission_request_id
                    && option.0 == selected_option_id
                    && authority.decision.0 == [41; 16]
                    && authority.subject.0 == [42; 16]
                    && authority.epoch == 43
            )
    ));
    drop(service);
    let _ = fs::remove_file(participation_socket);
    let _ = fs::remove_file(agent_socket);
    cleanup(&database, &root);
}

#[test]
fn interactive_permission_component_uses_one_connection_per_real_daemon() {
    let _test_lock = TEST_LOCK.lock().unwrap();
    let root = path("pec");
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
    let participation_worker = thread::spawn(move || participation_daemon.serve_next().unwrap());
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
        last_context: None,
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
    let participation_port = moira_tui_local_client::LocalAgentParticipationPort::connect(
        &participation_socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let agent_port = moira_tui_local_client::LocalAgentPort::connect(
        &agent_socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let mut component = moira_tui::PermissionControlComponent::new(
        participation_port,
        agent_port,
        binding.participation,
    );
    wait_permission(&mut component, "allow once");
    component.update(moira_tui::PermissionControlInput::Arm, true);
    component.update(moira_tui::PermissionControlInput::Confirm, true);
    wait_permission(&mut component, "Permission choice durably accepted");
    drop(component);
    participation_worker.join().unwrap();
    agent_worker.join().unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    cleanup(&database, &root);
}

fn wait_cancel(component: &mut moira_tui::CancellationControlComponent, needle: &str) {
    for _ in 0..200 {
        component.poll();
        if render_cancel(component).contains(needle) {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("cancellation component did not render {needle}");
}

fn render_cancel(component: &moira_tui::CancellationControlComponent) -> String {
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

fn wait_prompt(component: &mut moira_tui::PromptControlComponent, needle: &str) {
    for _ in 0..200 {
        component.poll();
        if render_prompt(component).contains(needle) {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("prompt component did not render {needle}");
}

fn render_prompt(component: &moira_tui::PromptControlComponent) -> String {
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

fn wait_permission(component: &mut moira_tui::PermissionControlComponent, needle: &str) {
    let mut rendered = String::new();
    for _ in 0..200 {
        component.poll();
        rendered = render_permission(component);
        if rendered.contains(needle) {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("permission component did not render {needle}: {rendered}");
}

fn render_permission(component: &moira_tui::PermissionControlComponent) -> String {
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

fn wait_agent(component: &mut moira_tui::AgentComponent, needle: &str) {
    for _ in 0..200 {
        component.poll();
        if render_agent(component).contains(needle) {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("agent component did not render {needle}");
}

fn render_agent(component: &moira_tui::AgentComponent) -> String {
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

fn prime_ready(database: &Path, binding: p::BindingV1) {
    let mut service = AgentParticipationService::new(
        AgentParticipationSqliteStore::open(database, [1; 16], limits()).unwrap(),
    );
    apply(&mut service, None, p::MutationV1::Admit(binding));
    let open = p::OperationV1::Open {
        key: p::IdempotencyKeyV1([51; 16]),
        execution: p::ExecutionIdV1([52; 16]),
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
            key: p::IdempotencyKeyV1([51; 16]),
            observation: observation(p::PhaseV1::Ready, 1, None, None),
        },
    );
}

fn prime_reattach(database: &Path, binding: p::BindingV1) {
    let mut service = AgentParticipationService::new(
        AgentParticipationSqliteStore::open(database, [1; 16], limits()).unwrap(),
    );
    apply(&mut service, None, p::MutationV1::Admit(binding));
    let open = p::OperationV1::Open {
        key: p::IdempotencyKeyV1([53; 16]),
        execution: p::ExecutionIdV1([54; 16]),
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
    let mut ready = observation(p::PhaseV1::Ready, 1, None, None);
    ready.capabilities.resume_session = true;
    apply(
        &mut service,
        Some(1),
        p::MutationV1::Observe {
            participation: binding.participation,
            expected_revision: 1,
            key: p::IdempotencyKeyV1([53; 16]),
            observation: ready,
        },
    );
    apply(
        &mut service,
        Some(2),
        p::MutationV1::MarkReattachRequired {
            participation: binding.participation,
            expected_revision: 2,
            session_revision: 1,
        },
    );
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

fn write_wire_frame(stream: &mut UnixStream, bytes: &[u8]) {
    stream
        .write_all(&u32::try_from(bytes.len()).unwrap().to_be_bytes())
        .unwrap();
    stream.write_all(bytes).unwrap();
    stream.flush().unwrap();
}

fn write_ready(stream: &mut UnixStream, requests: u32) {
    let ready = encode_ready(
        ReadyV1 {
            protocol_version: 1,
            max_frame_bytes: 64 * 1024,
            max_requests: requests,
        },
        LimitsV1::default(),
    )
    .unwrap();
    write_wire_frame(stream, &ready);
}

fn write_participation_reply(stream: &mut UnixStream, reply: &p::ReplyV1) {
    let bytes = encode_agent_participation_reply(reply, LimitsV1::default()).unwrap();
    write_wire_frame(stream, &bytes);
}

fn read_wire_frame(stream: &mut UnixStream) -> Vec<u8> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).unwrap();
    let mut bytes = vec![0; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut bytes).unwrap();
    bytes
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

#![cfg(unix)]

use moira_core::saga::{SagaLimitsV1, persistence::SagaCodecLimitsV1};
use moira_local_daemon::unix::{SagaAuthenticator, SagaDaemon};
use moira_protocol::{
    OperatorOperationV1, SagaCommandV1, SagaEffectOutcomeV1, SagaMutationV1, SagaOutcomeV1,
    SagaReplyV1, SagaSourceV1, SagaTransitionV1,
};
use moira_service::{
    AccessV1,
    saga::{OperatorAccessV1, SagaService},
};
use moira_sqlite::saga::{SagaSqliteStore, SagaStoreLimitsV1};
use moira_wire::LimitsV1;
use moira_wire::{ReadyV1, decode_saga_request, encode_ready, encode_saga_reply};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::Value;
use std::{
    convert::Infallible,
    fs,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Authentication;

impl SagaAuthenticator for Authentication {
    type Error = Infallible;

    fn authenticate(&mut self, _: &UnixStream) -> Result<AccessV1, Self::Error> {
        Ok(AccessV1 {
            read: true,
            mutate: true,
        })
    }

    fn authorize_operator(
        &mut self,
        _: OperatorOperationV1,
    ) -> Result<Option<OperatorAccessV1>, Self::Error> {
        Ok(Some(OperatorAccessV1 {
            subject: [4; 16],
            authority_decision: 5,
            authority_epoch: 6,
        }))
    }
}

#[test]
fn interactive_control_recovers_a_lost_reply_from_the_exact_durable_receipt() {
    let root = std::env::temp_dir().join(format!(
        "moira-tui-reconciliation-lost-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let database = root.join("saga.db");
    let socket = root.join("saga.sock");
    let (mut service, saga) = seeded_service(&database);
    let listener = UnixListener::bind(&socket).unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        ready(&mut stream, 8);
        loop {
            let command =
                decode_saga_request(&read_frame(&mut stream), LimitsV1::default()).unwrap();
            if let SagaCommandV1::ConfirmReconciliation(selected) = command {
                let (expected_revision, operation) =
                    service.prepare_confirmed_reconciliation(selected).unwrap();
                let reply = service
                    .execute_operator_authorized(
                        OperatorAccessV1 {
                            subject: [4; 16],
                            authority_decision: 5,
                            authority_epoch: 6,
                        },
                        SagaCommandV1::Reconcile {
                            expected_revision,
                            operation,
                        },
                    )
                    .unwrap();
                assert!(matches!(
                    reply,
                    SagaReplyV1::Accepted(SagaOutcomeV1::Applied {
                        replayed: false,
                        ..
                    })
                ));
                drop(stream);
                break;
            }
            let reply = service
                .execute_authorized(
                    AccessV1 {
                        read: true,
                        mutate: false,
                    },
                    command,
                )
                .unwrap();
            write_reply(&mut stream, &reply);
        }

        let (mut recovery, _) = listener.accept().unwrap();
        ready(&mut recovery, 8);
        let command = decode_saga_request(&read_frame(&mut recovery), LimitsV1::default()).unwrap();
        assert!(matches!(command, SagaCommandV1::OperatorReceipt { .. }));
        let reply = service
            .execute_authorized(
                AccessV1 {
                    read: true,
                    mutate: false,
                },
                command,
            )
            .unwrap();
        write_reply(&mut recovery, &reply);
    });

    let port = moira_tui_local_client::LocalSagaPort::connect(
        &socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let mut component = moira_tui::ReconciliationControlComponent::new(
        port,
        moira_protocol::ReconciliationProposalV1 {
            saga,
            disposition: moira_protocol::OperatorDispositionV1::KeepUncertain,
        },
    );
    wait_for_component(&mut component, "server challenge:");
    component.update(moira_tui::ReconciliationControlInput::Arm, true);
    component.update(moira_tui::ReconciliationControlInput::Confirm, true);
    wait_for_component(&mut component, "OUTCOME UNKNOWN");
    component.update(moira_tui::ReconciliationControlInput::Refresh, true);
    wait_for_component(&mut component, "durable receipt:");
    assert!(render(&component).contains("Unknown outcome recovered"));
    drop(component);
    worker.join().unwrap();

    for suffix in ["", "-journal", ".owner.lock"] {
        let _ = fs::remove_file(format!("{}{}", database.display(), suffix));
    }
    let _ = fs::remove_file(socket);
    let _ = fs::remove_dir(root);
}

#[test]
fn cli_confirms_only_the_server_prepared_reconciliation() {
    let root = std::env::temp_dir().join(format!(
        "moira-tui-reconciliation-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let database = root.join("saga.db");
    let socket = root.join("saga.sock");
    let limits = SagaStoreLimitsV1 {
        codec: SagaCodecLimitsV1 {
            max_bytes: 16_384,
            max_sagas: 8,
            max_operator_actions: 8,
        },
        saga: SagaLimitsV1 {
            max_sagas: 8,
            max_retries_per_saga: 2,
            max_inspection_page: 8,
            max_operator_actions: 8,
        },
        max_database_pages: 64,
    };
    let mut service = SagaService::new(SagaSqliteStore::open(&database, [1; 16], limits).unwrap());
    let SagaReplyV1::Accepted(SagaOutcomeV1::Applied {
        transition: SagaTransitionV1::Admitted { saga, effect, .. },
        ..
    }) = service
        .execute(SagaCommandV1::Apply {
            expected_revision: 0,
            mutation: SagaMutationV1::Admit {
                source: SagaSourceV1([2; 16]),
                now: 10,
                deadline: 20,
                retry_budget: 2,
            },
        })
        .unwrap()
    else {
        panic!("admission expected")
    };
    service
        .execute(SagaCommandV1::Apply {
            expected_revision: 1,
            mutation: SagaMutationV1::AcceptEffect {
                saga,
                effect,
                outcome: SagaEffectOutcomeV1::Uncertain,
                observed_at: 11,
            },
        })
        .unwrap();
    let mut daemon = SagaDaemon::bind_new(
        &socket,
        service,
        Authentication,
        LimitsV1::default(),
        Duration::from_secs(3),
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
        .args([
            "reconciliation-view",
            socket.to_str().unwrap(),
            "0",
            "keep-uncertain",
        ])
        .output()
        .unwrap();
    assert!(viewed.status.success(), "{:?}", viewed);
    let view: Value = serde_json::from_slice(&viewed.stdout).unwrap();
    assert_eq!(view["status"], "current");
    assert_eq!(view["storage_revision"], 2);
    let action = view["action"].as_str().unwrap();
    let digest = view["confirmation_digest"].as_str().unwrap();

    let port = moira_tui_local_client::LocalSagaPort::connect(
        &socket,
        moira_tui_local_client::ClientLimits::default(),
    )
    .unwrap();
    let mut component = moira_tui::ReconciliationControlComponent::new(
        port,
        moira_protocol::ReconciliationProposalV1 {
            saga,
            disposition: moira_protocol::OperatorDispositionV1::KeepUncertain,
        },
    );
    for _ in 0..100 {
        component.poll();
        if render(&component).contains("server challenge:") {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let rendered = render(&component);
    assert!(rendered.contains("server challenge:"), "{rendered}");
    assert!(rendered.contains("proposed disposition: KeepUncertain"));
    drop(component);

    let confirmed = Command::new(binary)
        .args([
            "reconciliation-confirm",
            socket.to_str().unwrap(),
            "2",
            "0",
            "keep-uncertain",
            action,
            digest,
            "CONFIRM",
        ])
        .output()
        .unwrap();
    assert!(confirmed.status.success(), "{:?}", confirmed);
    let result: Value = serde_json::from_slice(&confirmed.stdout).unwrap();
    assert_eq!(result["status"], "accepted");
    assert_eq!(result["action"], action);
    assert_eq!(result["replayed"], false);
    worker.join().unwrap();

    for suffix in ["", "-journal", ".owner.lock"] {
        let _ = fs::remove_file(format!("{}{}", database.display(), suffix));
    }
    let _ = fs::remove_dir(&root);
}

fn render(component: &moira_tui::ReconciliationControlComponent) -> String {
    let mut terminal = Terminal::new(TestBackend::new(160, 18)).unwrap();
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

fn wait_for_component(component: &mut moira_tui::ReconciliationControlComponent, needle: &str) {
    for _ in 0..200 {
        component.poll();
        if render(component).contains(needle) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("component did not render {needle}: {}", render(component));
}

fn seeded_service(
    database: &std::path::Path,
) -> (SagaService<SagaSqliteStore>, moira_protocol::SagaIdV1) {
    let limits = SagaStoreLimitsV1 {
        codec: SagaCodecLimitsV1 {
            max_bytes: 16_384,
            max_sagas: 8,
            max_operator_actions: 8,
        },
        saga: SagaLimitsV1 {
            max_sagas: 8,
            max_retries_per_saga: 2,
            max_inspection_page: 8,
            max_operator_actions: 8,
        },
        max_database_pages: 64,
    };
    let mut service = SagaService::new(SagaSqliteStore::open(database, [1; 16], limits).unwrap());
    let SagaReplyV1::Accepted(SagaOutcomeV1::Applied {
        transition: SagaTransitionV1::Admitted { saga, effect, .. },
        ..
    }) = service
        .execute(SagaCommandV1::Apply {
            expected_revision: 0,
            mutation: SagaMutationV1::Admit {
                source: SagaSourceV1([2; 16]),
                now: 10,
                deadline: 20,
                retry_budget: 2,
            },
        })
        .unwrap()
    else {
        panic!("admission expected")
    };
    service
        .execute(SagaCommandV1::Apply {
            expected_revision: 1,
            mutation: SagaMutationV1::AcceptEffect {
                saga,
                effect,
                outcome: SagaEffectOutcomeV1::Uncertain,
                observed_at: 11,
            },
        })
        .unwrap();
    (service, saga)
}

fn ready(stream: &mut UnixStream, max_requests: u32) {
    let bytes = encode_ready(
        ReadyV1 {
            protocol_version: 1,
            max_frame_bytes: 64 * 1024,
            max_requests,
        },
        LimitsV1::default(),
    )
    .unwrap();
    write_frame(stream, &bytes);
}

fn read_frame(stream: &mut UnixStream) -> Vec<u8> {
    let mut header = [0; 4];
    stream.read_exact(&mut header).unwrap();
    let mut bytes = vec![0; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut bytes).unwrap();
    bytes
}

fn write_reply(stream: &mut UnixStream, reply: &SagaReplyV1) {
    let bytes = encode_saga_reply(reply, LimitsV1::default()).unwrap();
    write_frame(stream, &bytes);
}

fn write_frame(stream: &mut UnixStream, bytes: &[u8]) {
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(bytes).unwrap();
    stream.flush().unwrap();
}

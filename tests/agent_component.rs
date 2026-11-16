use std::{thread, time::Duration};

use moira_client::AgentServicePort;
use moira_protocol::agent as a;
use ratatui::{Terminal, backend::TestBackend};

struct Port {
    step: usize,
    mode: PageMode,
}

#[derive(Clone, Copy)]
enum PageMode {
    Complete,
    Corrupt,
    Truncated,
    Recovery,
}

impl AgentServicePort for Port {
    type Error = ();

    fn execute(&mut self, command: a::CommandV1) -> Result<a::ReplyV1, Self::Error> {
        let cycle = self.step / 3;
        let expected = match self.step % 3 {
            0 | 2 => a::CommandV1::Inspect { session: session() },
            1 => a::CommandV1::Events {
                session: session(),
                after: (matches!(self.mode, PageMode::Truncated) && cycle > 0).then_some(63),
                limit: 64,
            },
            _ => unreachable!(),
        };
        assert_eq!(command, expected);
        self.step += 1;
        Ok(a::ReplyV1::Accepted(if self.step % 3 == 2 {
            a::OutcomeV1::Events(page(self.mode, cycle))
        } else {
            a::OutcomeV1::Session(view(self.mode))
        }))
    }
}

#[test]
fn component_double_reads_and_renders_typed_byte_safe_events() {
    let mut component = moira_tui::AgentComponent::new(
        Port {
            step: 0,
            mode: PageMode::Complete,
        },
        session(),
    );
    for _ in 0..100 {
        component.poll();
        let rendered = render(&component);
        if rendered.contains("Stable agent revision 7") {
            assert!(rendered.contains("phase: AwaitingPermission"));
            assert!(rendered.contains("events shown: 3 • complete"));
            assert!(rendered.contains("assistant: hello\\x00world"));
            assert!(rendered.contains("tool: cargo test • bounded workspace"));
            assert!(rendered.contains("permission[1 option(s)]"));
            assert!(rendered.contains("permission=true read=false write=false terminal=false"));
            assert!(rendered.contains("load=true resume=true modes=true models=true"));
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("agent component did not render its stable typed projection");
}

#[test]
fn component_refuses_a_noncontiguous_event_page() {
    let mut component = moira_tui::AgentComponent::new(
        Port {
            step: 0,
            mode: PageMode::Corrupt,
        },
        session(),
    );
    for _ in 0..100 {
        component.poll();
        let rendered = render(&component);
        if rendered.contains("Invalid Moira agent reply") {
            assert!(rendered.contains("No stable provider-neutral agent session"));
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("agent component admitted a noncontiguous event page");
}

#[test]
fn component_marks_a_valid_bounded_prefix_as_truncated() {
    let mut component = moira_tui::AgentComponent::new(
        Port {
            step: 0,
            mode: PageMode::Truncated,
        },
        session(),
    );
    let mut observed_prefix = false;
    for _ in 0..100 {
        component.poll();
        let rendered = render(&component);
        if rendered.contains("Stable agent revision 7") {
            assert!(rendered.contains("events shown: 64 • TRUNCATED"));
            assert!(rendered.contains("last cursor: Some(64)"));
            observed_prefix = true;
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(observed_prefix, "agent component did not label truncation");
    component.update(moira_tui::Input::Open, true);
    for _ in 0..100 {
        component.poll();
        let rendered = render(&component);
        if rendered.contains("after: Some(63) • events shown: 1 • complete") {
            assert!(rendered.contains("64 assistant:"));
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("agent component did not traverse to the next bounded page");
}

#[test]
fn component_labels_recovery_without_offering_automatic_resume() {
    let mut component = moira_tui::AgentComponent::new(
        Port {
            step: 0,
            mode: PageMode::Recovery,
        },
        session(),
    );
    for _ in 0..100 {
        component.poll();
        let rendered = render(&component);
        if rendered.contains("Stable agent revision 7") {
            assert!(rendered.contains("phase: ReattachRequired"));
            assert!(rendered.contains("explicit governed reattach required"));
            assert!(rendered.contains("automatic resume disabled"));
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("agent component did not render its recovery fence");
}

fn render(component: &moira_tui::AgentComponent) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 16)).unwrap();
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

fn session() -> a::SessionIdV1 {
    a::SessionIdV1([1; 16])
}

fn view(mode: PageMode) -> a::SessionViewV1 {
    a::SessionViewV1 {
        session: session(),
        revision: 7,
        phase: if matches!(mode, PageMode::Recovery) {
            a::PhaseV1::ReattachRequired
        } else {
            a::PhaseV1::AwaitingPermission
        },
        profile_document_digest: a::DigestV1([2; 32]),
        active_turn: (!matches!(mode, PageMode::Recovery)).then_some(a::TurnIdV1([3; 16])),
        last_event_cursor: match mode {
            PageMode::Complete | PageMode::Corrupt => Some(2),
            PageMode::Truncated => Some(64),
            PageMode::Recovery => None,
        },
        capabilities: a::CapabilitiesV1 {
            load_session: true,
            resume_session: true,
            permissions: true,
            session_modes: true,
            session_models: true,
            ..a::CapabilitiesV1::default()
        },
    }
}

fn page(mode: PageMode, cycle: usize) -> a::EventPageV1 {
    if matches!(mode, PageMode::Recovery) {
        return a::EventPageV1 {
            session: session(),
            entries: Vec::new(),
            next_after: None,
        };
    }
    if matches!(mode, PageMode::Truncated) {
        if cycle > 0 {
            return a::EventPageV1 {
                session: session(),
                entries: vec![a::EventRecordV1 {
                    cursor: 64,
                    event: a::EventV1::AssistantMessage {
                        bytes: b"tail".to_vec(),
                    },
                }],
                next_after: None,
            };
        }
        return a::EventPageV1 {
            session: session(),
            entries: (0..64)
                .map(|cursor| a::EventRecordV1 {
                    cursor,
                    event: a::EventV1::AssistantMessage { bytes: Vec::new() },
                })
                .collect(),
            next_after: Some(63),
        };
    }
    a::EventPageV1 {
        session: session(),
        entries: vec![
            a::EventRecordV1 {
                cursor: 0,
                event: a::EventV1::AssistantMessage {
                    bytes: b"hello\0world".to_vec(),
                },
            },
            a::EventRecordV1 {
                cursor: if matches!(mode, PageMode::Corrupt) {
                    9
                } else {
                    1
                },
                event: a::EventV1::ToolCall {
                    title: b"cargo test".to_vec(),
                    detail: b"bounded workspace".to_vec(),
                },
            },
            a::EventRecordV1 {
                cursor: 2,
                event: a::EventV1::Permission(a::PermissionRequestV1 {
                    request: a::PermissionRequestIdV1([4; 16]),
                    turn: a::TurnIdV1([3; 16]),
                    capability: b"terminal.execute".to_vec(),
                    description: b"run bounded tests".to_vec(),
                    options: vec![a::PermissionOptionV1 {
                        id: a::PermissionOptionIdV1([5; 16]),
                        kind: a::PermissionOptionKindV1::AllowOnce,
                        label: b"allow once".to_vec(),
                    }],
                }),
            },
        ],
        next_after: None,
    }
}

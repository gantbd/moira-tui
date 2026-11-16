//! Standalone interactive and headless Moira terminal application.
#![forbid(unsafe_code)]

#[cfg(unix)]
mod cancel_confirm;
#[cfg(unix)]
mod cancel_view;
#[cfg(unix)]
mod permission_confirm;
#[cfg(unix)]
mod permission_view;

#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::path::PathBuf;

    let mut args = std::env::args_os().skip(1);
    let Some(mode) = args.next() else {
        return Err(usage().into());
    };
    if mode == "--help" || mode == "-h" {
        println!("{}", usage());
        return Ok(());
    }
    if mode == "capabilities" {
        if args.next().is_some() {
            return Err(usage().into());
        }
        let manifest = moira_tui::CAPABILITIES;
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema": manifest.schema,
                "peer": manifest.peer,
                "interactive": manifest.interactive,
                "headless": manifest.headless,
            }))?
        );
        return Ok(());
    }
    if mode == "agent-inspect" || mode == "agent-events" {
        let socket = args.next().map(PathBuf::from).ok_or_else(|| usage())?;
        let session = args
            .next()
            .and_then(|value| parse_id(&value))
            .ok_or_else(|| usage())?;
        let after = if mode == "agent-events" {
            match args.next() {
                Some(value) if value == "-" => None,
                Some(value) => Some(value.to_string_lossy().parse::<u64>()?),
                None => None,
            }
        } else {
            None
        };
        if args.next().is_some() {
            return Err(usage().into());
        }
        let port = moira_tui_local_client::LocalAgentPort::connect(
            socket,
            moira_tui_local_client::ClientLimits::default(),
        )
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        return run_agent_headless(port, mode == "agent-events", session, after);
    }
    if mode == "participation-inspect" {
        let socket = args.next().map(PathBuf::from).ok_or_else(|| usage())?;
        let participation = args
            .next()
            .and_then(|value| parse_bytes(&value))
            .map(moira_protocol::agent_participation::ParticipationIdV1)
            .ok_or_else(|| usage())?;
        if args.next().is_some() {
            return Err(usage().into());
        }
        let port = moira_tui_local_client::LocalAgentParticipationPort::connect(
            socket,
            moira_tui_local_client::ClientLimits::default(),
        )
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        return run_participation_headless(port, participation);
    }
    if mode == "permission-view" {
        let participation_socket = args.next().map(PathBuf::from).ok_or_else(|| usage())?;
        let agent_socket = args.next().map(PathBuf::from).ok_or_else(|| usage())?;
        let participation = args
            .next()
            .and_then(|value| parse_bytes(&value))
            .map(moira_protocol::agent_participation::ParticipationIdV1)
            .ok_or_else(|| usage())?;
        if args.next().is_some() {
            return Err(usage().into());
        }
        let participation_port = moira_tui_local_client::LocalAgentParticipationPort::connect(
            participation_socket,
            moira_tui_local_client::ClientLimits::default(),
        )
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        let agent_port = moira_tui_local_client::LocalAgentPort::connect(
            agent_socket,
            moira_tui_local_client::ClientLimits::default(),
        )
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        return permission_view::run(participation_port, agent_port, participation);
    }
    if mode == "permission-confirm" {
        let socket = args.next().map(PathBuf::from).ok_or_else(|| usage())?;
        let participation = args
            .next()
            .and_then(|value| parse_bytes(&value))
            .map(moira_protocol::agent_participation::ParticipationIdV1)
            .ok_or_else(|| usage())?;
        let option = args
            .next()
            .and_then(|value| parse_bytes(&value))
            .map(moira_protocol::agent_participation::PermissionOptionIdV1)
            .ok_or_else(|| usage())?;
        let confirmation_digest = args
            .next()
            .and_then(|value| parse_digest(&value))
            .ok_or_else(|| usage())?;
        if args.next().as_deref() != Some(std::ffi::OsStr::new("CONFIRM")) || args.next().is_some()
        {
            return Err(usage().into());
        }
        let port = moira_tui_local_client::LocalAgentParticipationPort::connect(
            socket,
            moira_tui_local_client::ClientLimits::default(),
        )
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        return permission_confirm::run(
            port,
            moira_protocol::agent_participation::ConfirmedPermissionSelectionV1 {
                participation,
                confirmation_digest,
                option,
            },
        );
    }
    if mode == "cancel-view" || mode == "cancel-confirm" {
        let socket = args.next().map(PathBuf::from).ok_or_else(|| usage())?;
        let participation = args
            .next()
            .and_then(|value| parse_bytes(&value))
            .map(moira_protocol::agent_participation::ParticipationIdV1)
            .ok_or_else(|| usage())?;
        if mode == "cancel-view" {
            if args.next().is_some() {
                return Err(usage().into());
            }
            let port = moira_tui_local_client::LocalAgentParticipationPort::connect(
                socket,
                moira_tui_local_client::ClientLimits::default(),
            )
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
            return cancel_view::run(port, participation);
        }
        let confirmation_digest = args
            .next()
            .and_then(|value| parse_digest(&value))
            .ok_or_else(|| usage())?;
        if args.next().as_deref() != Some(std::ffi::OsStr::new("CONFIRM")) || args.next().is_some()
        {
            return Err(usage().into());
        }
        let port = moira_tui_local_client::LocalAgentParticipationPort::connect(
            socket,
            moira_tui_local_client::ClientLimits::default(),
        )
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        return cancel_confirm::run(
            port,
            moira_protocol::agent_participation::ConfirmedCancelV1 {
                participation,
                confirmation_digest,
            },
        );
    }
    if mode != "interactive" && mode != "headless" {
        return Err(usage().into());
    }
    let socket = match args.next().map(PathBuf::from) {
        Some(socket) if args.next().is_none() => socket,
        _ if mode == "headless" => {
            return headless_failure("configuration_refused", usage());
        }
        _ => return Err(usage().into()),
    };
    let port = match moira_tui_local_client::LocalSagaPort::connect(
        socket,
        moira_tui_local_client::ClientLimits::default(),
    ) {
        Ok(port) => port,
        Err(error) if mode == "headless" => {
            return headless_failure("connect_refused", &format!("{error:?}"));
        }
        Err(error) => return Err(std::io::Error::other(format!("{error:?}")).into()),
    };
    if mode == "interactive" {
        run_interactive(moira_tui::Component::new(port))
    } else {
        run_headless(port)
    }
}

#[cfg(unix)]
fn run_agent_headless(
    port: moira_tui_local_client::LocalAgentPort,
    events: bool,
    session: moira_protocol::agent::SessionIdV1,
    after: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    use moira_client::AgentReadOnlyClient;
    use moira_protocol::agent::{OutcomeV1, ReplyV1};
    let mut client = AgentReadOnlyClient::new(port);
    let reply = if events {
        client.events(session, after, 64)
    } else {
        client.inspect(session)
    }
    .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let value = match reply {
        ReplyV1::Accepted(OutcomeV1::Session(view)) => serde_json::json!({
            "version": 1, "capability": "agent.session.inspect.v1", "status": "current",
            "revision": view.revision, "phase": format!("{:?}", view.phase),
            "active_turn": view.active_turn.map(|value| format!("{value:?}")),
            "last_event_cursor": view.last_event_cursor,
            "capabilities": {
                "load_session": view.capabilities.load_session, "resume_session": view.capabilities.resume_session,
                "permissions": view.capabilities.permissions, "filesystem_read": view.capabilities.filesystem_read,
                "filesystem_write": view.capabilities.filesystem_write, "terminal": view.capabilities.terminal,
                "session_modes": view.capabilities.session_modes, "session_models": view.capabilities.session_models,
            }
        }),
        ReplyV1::Accepted(OutcomeV1::Events(page)) => serde_json::json!({
            "version": 1, "capability": "agent.events.observe.v1", "status": "current",
            "events": page.entries.into_iter().map(event_json).collect::<Vec<_>>(),
            "next_after": page.next_after,
        }),
        ReplyV1::Accepted(OutcomeV1::OutcomeUnknown { .. }) => serde_json::json!({
            "version": 1, "capability": if events { "agent.events.observe.v1" } else { "agent.session.inspect.v1" },
            "status": "outcome_unknown"
        }),
        ReplyV1::Refused(refusal) => serde_json::json!({
            "version": 1, "capability": if events { "agent.events.observe.v1" } else { "agent.session.inspect.v1" },
            "status": "refused", "refusal": format!("{refusal:?}")
        }),
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

#[cfg(unix)]
fn event_json(record: moira_protocol::agent::EventRecordV1) -> serde_json::Value {
    use moira_protocol::agent::EventV1;
    let event = match record.event {
        EventV1::AssistantMessage { bytes } => {
            serde_json::json!({"kind":"assistant_message","bytes":bytes})
        }
        EventV1::Thought { bytes } => {
            serde_json::json!({"kind":"thought","bytes":bytes})
        }
        EventV1::Plan { bytes } => {
            serde_json::json!({"kind":"plan","bytes":bytes})
        }
        EventV1::ToolCall { title, detail } => {
            serde_json::json!({"kind":"tool_call","title":title,"detail":detail})
        }
        EventV1::Terminal { stream, bytes } => {
            serde_json::json!({"kind":"terminal","stream":stream,"bytes":bytes})
        }
        EventV1::Permission(value) => {
            serde_json::json!({"kind":"permission","request":format!("{:?}",value.request),"capability":value.capability,"description":value.description,"options":value.options.into_iter().map(|option|serde_json::json!({"id":format!("{:?}",option.id),"kind":format!("{:?}",option.kind),"label":option.label})).collect::<Vec<_>>() })
        }
        EventV1::FileChange {
            path,
            content_digest,
        } => {
            serde_json::json!({"kind":"file_change","path":path,"content_digest":format!("{content_digest:?}")})
        }
        EventV1::Usage {
            input_tokens,
            output_tokens,
        } => {
            serde_json::json!({"kind":"usage","input_tokens":input_tokens,"output_tokens":output_tokens})
        }
        EventV1::TurnFinished {
            disposition,
            stop_reason,
            output_digest,
            ..
        } => {
            serde_json::json!({"kind":"turn_finished","disposition":format!("{disposition:?}"),"stop_reason":format!("{stop_reason:?}"),"output_digest":output_digest.map(|value|format!("{value:?}"))})
        }
    };
    serde_json::json!({"cursor":record.cursor,"event":event})
}

#[cfg(unix)]
fn run_participation_headless(
    port: moira_tui_local_client::LocalAgentParticipationPort,
    participation: moira_protocol::agent_participation::ParticipationIdV1,
) -> Result<(), Box<dyn std::error::Error>> {
    use moira_client::AgentParticipationReadOnlyClient;
    use moira_protocol::agent_participation::{OutcomeV1, ReplyV1};

    let mut client = AgentParticipationReadOnlyClient::new(port);
    let reply = client
        .participation(participation)
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let value = match reply {
        ReplyV1::Accepted(value) => match *value {
            OutcomeV1::Participation(Some(view)) => serde_json::json!({
                "version": 1,
                "capability": "agent.participation.inspect.v1",
                "status": "current",
                "revision": view.revision,
                "phase": format!("{:?}", view.phase),
                "session_revision": view.session_revision,
                "last_event_cursor": view.last_event_cursor,
                "persona": format!("{:?}", view.binding.persona),
                "grounding_depth": format!("{:?}", view.binding.grounding.depth),
                "method_machine_digest": view.binding.method.machine_digest,
                "method_state_digest": view.binding.method.state_digest,
                "grounding_authority_digest": view.binding.grounding.authority_digest,
                "grounding_surface_digest": view.binding.grounding.surface_digest,
                "active_turn": view.active_turn.map(|value| format!("{value:?}")),
                "awaiting_permission": view.permission.is_some(),
                "terminal": view.terminal.map(|evidence| serde_json::json!({
                    "disposition": format!("{:?}", evidence.disposition),
                    "stop_reason": format!("{:?}", evidence.stop_reason),
                    "output_digest": evidence.output_digest,
                    "provider_evidence_digest": evidence.provider_evidence_digest,
                })),
                "pending_operation": view.pending.map(operation_kind),
                "capabilities": {
                    "load_session": view.capabilities.load_session,
                    "resume_session": view.capabilities.resume_session,
                    "permissions": view.capabilities.permissions,
                    "filesystem_read": view.capabilities.filesystem_read,
                    "filesystem_write": view.capabilities.filesystem_write,
                    "terminal": view.capabilities.terminal,
                    "session_modes": view.capabilities.session_modes,
                    "session_models": view.capabilities.session_models,
                }
            }),
            OutcomeV1::Participation(None) => serde_json::json!({
                "version": 1, "capability": "agent.participation.inspect.v1",
                "status": "not_found"
            }),
            _ => serde_json::json!({
                "version": 1, "capability": "agent.participation.inspect.v1",
                "status": "invalid_reply"
            }),
        },
        ReplyV1::Refused(refusal) => serde_json::json!({
            "version": 1, "capability": "agent.participation.inspect.v1",
            "status": "refused", "refusal": format!("{refusal:?}")
        }),
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

#[cfg(unix)]
fn operation_kind(value: moira_protocol::agent_participation::OperationV1) -> &'static str {
    use moira_protocol::agent_participation::OperationV1;
    match value {
        OperationV1::Open { .. } => "open",
        OperationV1::Prompt { .. } => "prompt",
        OperationV1::Advance { .. } => "advance",
        OperationV1::DecidePermission { .. } => "decide_permission",
        OperationV1::Cancel { .. } => "cancel",
        OperationV1::Close { .. } => "close",
    }
}

#[cfg(unix)]
fn parse_id(value: &std::ffi::OsStr) -> Option<moira_protocol::agent::SessionIdV1> {
    parse_bytes(value).map(moira_protocol::agent::SessionIdV1)
}

#[cfg(unix)]
fn parse_bytes(value: &std::ffi::OsStr) -> Option<[u8; 16]> {
    let value = value.to_str()?;
    if value.len() != 32 {
        return None;
    }
    let mut bytes = [0_u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

#[cfg(unix)]
fn parse_digest(value: &std::ffi::OsStr) -> Option<[u8; 32]> {
    let value = value.to_str()?;
    if value.len() != 64 {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

#[cfg(unix)]
fn run_interactive(mut component: moira_tui::Component) -> Result<(), Box<dyn std::error::Error>> {
    use crossterm::event::{self, Event};
    use std::time::Duration;

    ratatui::run(|terminal| -> Result<(), Box<dyn std::error::Error>> {
        loop {
            component.poll();
            terminal.draw(|frame| component.draw_in(frame, frame.area(), true))?;
            if !event::poll(Duration::from_millis(50))? {
                continue;
            }
            match event::read()? {
                Event::Resize(..) => {}
                Event::Key(key) => {
                    let Some(input) = moira_tui::translate_key(key) else {
                        continue;
                    };
                    if component.update(input, true) == moira_tui::Lifecycle::CloseRequested {
                        break;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    })
}

#[cfg(unix)]
fn run_headless(
    port: moira_tui_local_client::LocalSagaPort,
) -> Result<(), Box<dyn std::error::Error>> {
    use moira_client::SagaReadOnlyClient;
    use moira_protocol::{SagaOutcomeV1, SagaReplyV1};

    let mut client = SagaReadOnlyClient::new(port);
    let reply = match client.inspect(None, 64) {
        Ok(reply) => reply,
        Err(error) => return headless_failure(classify(error), "Moira inspection failed"),
    };
    match reply {
        SagaReplyV1::Accepted(SagaOutcomeV1::Page(page)) => {
            let census = match client.census() {
                Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Census(census))) => census,
                Ok(SagaReplyV1::Refused(refusal)) => {
                    return headless_failure("refused", &format!("Moira refused: {refusal:?}"));
                }
                Ok(SagaReplyV1::Accepted(_)) => {
                    return headless_failure("invalid_reply", "unexpected Moira census reply");
                }
                Err(error) => {
                    return headless_failure(classify(error), "Moira census failed");
                }
            };
            let manifest = moira_tui::CAPABILITIES;
            let sagas = page
                .entries
                .into_iter()
                .map(|saga| {
                    serde_json::json!({
                        "id": format!("{:?}", saga.saga),
                        "phase": format!("{:?}", saga.phase),
                        "deadline": saga.deadline,
                        "current_effect": saga.current_effect.map(|value| format!("{value:?}")),
                        "current_kind": saga.current_kind.map(|value| format!("{value:?}")),
                        "retries_used": saga.retries_used,
                        "retry_budget": saga.retry_budget,
                    })
                })
                .collect::<Vec<_>>();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "version": 1,
                    "mode": "headless",
                    "complete": true,
                    "peer": manifest.peer,
                    "capability": manifest.headless[0],
                    "status": "current",
                    "sagas": sagas,
                    "truncated": page.next_after.is_some(),
                    "census": {
                        "total": census.total,
                        "runnable_effects": census.runnable_effects,
                        "awaiting_decision": census.awaiting_decision,
                        "fenced": census.fenced,
                        "terminal": census.terminal,
                        "retries_consumed": census.retries_consumed,
                        "saturated": census.saturated,
                    },
                }))?
            );
            Ok(())
        }
        SagaReplyV1::Refused(refusal) => {
            headless_failure("refused", &format!("Moira refused: {refusal:?}"))
        }
        SagaReplyV1::Accepted(_) => headless_failure("invalid_reply", "unexpected Moira reply"),
    }
}

#[cfg(unix)]
fn classify(error: moira_tui_local_client::ClientError) -> &'static str {
    use moira_tui_local_client::ClientError;
    match error {
        ClientError::NotSent(_) => "not_sent",
        ClientError::OutcomeUnknown(_) => "outcome_unknown",
        ClientError::RequestLimit => "request_limit",
        ClientError::InvalidLimits | ClientError::InvalidReady | ClientError::InvalidReply => {
            "invalid_reply"
        }
    }
}

#[cfg(unix)]
fn headless_failure(status: &str, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = moira_tui::CAPABILITIES;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "version": 1,
            "mode": "headless",
            "complete": false,
            "peer": manifest.peer,
            "capability": manifest.headless[0],
            "status": status,
        }))?
    );
    Err(std::io::Error::other(message).into())
}

fn usage() -> &'static str {
    "usage: moira-tui capabilities | moira-tui <interactive|headless> SOCKET | moira-tui agent-inspect SOCKET SESSION_HEX | moira-tui agent-events SOCKET SESSION_HEX [AFTER|-] | moira-tui participation-inspect SOCKET PARTICIPATION_HEX | moira-tui permission-view PARTICIPATION_SOCKET AGENT_SOCKET PARTICIPATION_HEX | moira-tui permission-confirm PARTICIPATION_SOCKET PARTICIPATION_HEX OPTION_HEX CONFIRMATION_DIGEST_HEX CONFIRM | moira-tui cancel-view PARTICIPATION_SOCKET PARTICIPATION_HEX | moira-tui cancel-confirm PARTICIPATION_SOCKET PARTICIPATION_HEX CONFIRMATION_DIGEST_HEX CONFIRM\nagent commands use Moira's bounded public services; they never connect directly to a provider or carry authority credentials"
}

#[cfg(not(unix))]
fn main() {
    eprintln!("moira-tui's selected standalone client requires Unix");
    std::process::exit(1);
}

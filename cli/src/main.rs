//! Standalone interactive and headless Moira terminal application.
#![forbid(unsafe_code)]

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
    "usage: moira-tui capabilities | moira-tui <interactive|headless> SOCKET\ncapabilities prints the peer-owned presentation manifest without connecting; headless emits one bounded versioned sagas.inspect.v1 observation without constructing TUI state"
}

#[cfg(not(unix))]
fn main() {
    eprintln!("moira-tui's selected standalone client requires Unix");
    std::process::exit(1);
}

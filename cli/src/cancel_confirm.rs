//! Explicit submission of one server-prepared cancellation challenge.

use std::{error::Error, io};

use moira_client::AgentParticipationCancellationClient;
use moira_protocol::agent_participation as participation;
use moira_tui_local_client::{ClientError, LocalAgentParticipationPort};
use serde_json::{Value, json};

pub fn run(
    port: LocalAgentParticipationPort,
    confirmation: participation::ConfirmedCancelV1,
) -> Result<(), Box<dyn Error>> {
    let mut client = AgentParticipationCancellationClient::new(port);
    match client.confirm(confirmation) {
        Ok(participation::ReplyV1::Accepted(value)) => match *value {
            participation::OutcomeV1::Applied {
                storage_revision,
                participation,
                replayed,
            } => emit(json!({
                "version": 1,
                "capability": "agent.cancel.confirm.v1",
                "status": "accepted",
                "storage_revision": storage_revision,
                "participation_revision": participation.revision,
                "phase": format!("{:?}", participation.phase),
                "pending_operation": participation.pending.map(operation_kind),
                "replayed": replayed,
            })),
            _ => fail("invalid_reply", "unexpected cancellation reply"),
        },
        Ok(participation::ReplyV1::Refused(refusal)) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": 1,
                    "capability": "agent.cancel.confirm.v1",
                    "status": "refused",
                    "refusal": format!("{refusal:?}"),
                }))?
            );
            Err(io::Error::other("cancellation refused").into())
        }
        Err(ClientError::NotSent(kind)) => transport_failure("not_sent", kind, true),
        Err(ClientError::OutcomeUnknown(kind)) => transport_failure("outcome_unknown", kind, false),
        Err(ClientError::RequestLimit) => fail("request_limit", "request limit reached"),
        Err(ClientError::InvalidLimits | ClientError::InvalidReady | ClientError::InvalidReply) => {
            fail("invalid_reply", "invalid cancellation reply")
        }
    }
}

fn operation_kind(value: participation::OperationV1) -> &'static str {
    match value {
        participation::OperationV1::Cancel { .. } => "cancel",
        participation::OperationV1::Open { .. } => "open",
        participation::OperationV1::Prompt { .. } => "prompt",
        participation::OperationV1::Advance { .. } => "advance",
        participation::OperationV1::DecidePermission { .. } => "decide_permission",
        participation::OperationV1::Close { .. } => "close",
    }
}

fn transport_failure(
    status: &str,
    kind: io::ErrorKind,
    retry_safe: bool,
) -> Result<(), Box<dyn Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": 1,
            "capability": "agent.cancel.confirm.v1",
            "status": status,
            "retry_safe": retry_safe,
            "recovery": if retry_safe { "resubmit exact confirmation" } else { "inspect participation before any further action" },
        }))?
    );
    Err(io::Error::new(kind, status).into())
}

fn fail(status: &str, message: &str) -> Result<(), Box<dyn Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": 1,
            "capability": "agent.cancel.confirm.v1",
            "status": status,
        }))?
    );
    Err(io::Error::other(message).into())
}

fn emit(value: Value) -> Result<(), Box<dyn Error>> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

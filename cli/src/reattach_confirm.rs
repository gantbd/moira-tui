//! Explicit submission of one server-prepared reattach challenge.

use std::{error::Error, io};

use moira_client::AgentParticipationReattachClient;
use moira_protocol::agent_participation as participation;
use moira_tui_local_client::{ClientError, LocalAgentParticipationPort};
use serde_json::json;

pub fn run(
    port: LocalAgentParticipationPort,
    confirmation: participation::ConfirmedReattachV1,
) -> Result<(), Box<dyn Error>> {
    let mut client = AgentParticipationReattachClient::new(port);
    match client.confirm(confirmation) {
        Ok(participation::ReplyV1::Accepted(value)) => match *value {
            participation::OutcomeV1::Applied {
                storage_revision,
                participation,
                replayed,
            } => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "version": 1,
                        "capability": "agent.reattach.confirm.v1",
                        "status": "accepted",
                        "storage_revision": storage_revision,
                        "participation_revision": participation.revision,
                        "phase": format!("{:?}", participation.phase),
                        "pending_operation": "reattach",
                        "replayed": replayed,
                    }))?
                );
                Ok(())
            }
            _ => fail("invalid_reply", "unexpected reattach reply"),
        },
        Ok(participation::ReplyV1::Refused(refusal)) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": 1,
                    "capability": "agent.reattach.confirm.v1",
                    "status": "refused",
                    "refusal": format!("{refusal:?}"),
                }))?
            );
            Err(io::Error::other("reattach refused").into())
        }
        Err(ClientError::NotSent(kind)) => transport_failure("not_sent", kind, true),
        Err(ClientError::OutcomeUnknown(kind)) => transport_failure("outcome_unknown", kind, false),
        Err(ClientError::RequestLimit) => fail("request_limit", "request limit reached"),
        Err(ClientError::InvalidLimits | ClientError::InvalidReady | ClientError::InvalidReply) => {
            fail("invalid_reply", "invalid reattach reply")
        }
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
            "capability": "agent.reattach.confirm.v1",
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
            "capability": "agent.reattach.confirm.v1",
            "status": status,
        }))?
    );
    Err(io::Error::other(message).into())
}

//! Explicit bounded submission of prompt bytes through Moira.

use std::{
    error::Error,
    ffi::OsStr,
    fs::File,
    io::{self, Read},
};

use moira_client::AgentParticipationPromptClient;
use moira_protocol::agent_participation as participation;
use moira_tui_local_client::{ClientError, LocalAgentParticipationPort};
use serde_json::{Value, json};

const MAX_PROMPT_INPUT_BYTES: usize = 60 * 1024;

pub fn read_prompt(source: &OsStr) -> Result<Vec<u8>, Box<dyn Error>> {
    let reader: Box<dyn Read> = if source == OsStr::new("-") {
        Box::new(io::stdin().lock())
    } else {
        Box::new(File::open(source)?)
    };
    let limit = u64::try_from(MAX_PROMPT_INPUT_BYTES + 1)?;
    let mut bytes = Vec::new();
    reader.take(limit).read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() > MAX_PROMPT_INPUT_BYTES {
        return fail(
            "invalid_prompt",
            "prompt must contain 1 through 61440 bytes",
        );
    }
    Ok(bytes)
}

pub fn run(
    port: LocalAgentParticipationPort,
    participation: participation::ParticipationIdV1,
    prompt: Vec<u8>,
) -> Result<(), Box<dyn Error>> {
    let mut client = AgentParticipationPromptClient::new(port);
    match client.submit(participation, prompt) {
        Ok(participation::ReplyV1::Accepted(value)) => match *value {
            participation::OutcomeV1::Applied {
                storage_revision,
                participation,
                replayed,
            } => emit(json!({
                "version": 1,
                "capability": "agent.prompt.submit.v1",
                "status": "accepted",
                "storage_revision": storage_revision,
                "participation_revision": participation.revision,
                "phase": format!("{:?}", participation.phase),
                "pending_operation": participation.pending.map(operation_kind),
                "replayed": replayed,
            })),
            _ => fail("invalid_reply", "unexpected prompt submission reply"),
        },
        Ok(participation::ReplyV1::Refused(refusal)) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": 1,
                    "capability": "agent.prompt.submit.v1",
                    "status": "refused",
                    "refusal": format!("{refusal:?}"),
                }))?
            );
            Err(io::Error::other("prompt submission refused").into())
        }
        Err(ClientError::NotSent(kind)) => transport_failure("not_sent", kind, true),
        Err(ClientError::OutcomeUnknown(kind)) => transport_failure("outcome_unknown", kind, false),
        Err(ClientError::RequestLimit) => fail("request_limit", "request limit reached"),
        Err(ClientError::InvalidLimits | ClientError::InvalidReady | ClientError::InvalidReply) => {
            fail("invalid_reply", "invalid prompt submission reply")
        }
    }
}

fn operation_kind(value: participation::OperationV1) -> &'static str {
    match value {
        participation::OperationV1::Prompt { .. } => "prompt",
        participation::OperationV1::Open { .. } => "open",
        participation::OperationV1::Advance { .. } => "advance",
        participation::OperationV1::DecidePermission { .. } => "decide_permission",
        participation::OperationV1::Cancel { .. } => "cancel",
        participation::OperationV1::Close { .. } => "close",
        participation::OperationV1::Reattach { .. } => "reattach",
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
            "capability": "agent.prompt.submit.v1",
            "status": status,
            "retry_safe": retry_safe,
            "recovery": if retry_safe { "resubmit the exact prompt bytes" } else { "inspect participation before any further action" },
        }))?
    );
    Err(io::Error::new(kind, status).into())
}

fn fail<T>(status: &str, message: &str) -> Result<T, Box<dyn Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": 1,
            "capability": "agent.prompt.submit.v1",
            "status": status,
        }))?
    );
    Err(io::Error::other(message).into())
}

fn emit(value: Value) -> Result<(), Box<dyn Error>> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

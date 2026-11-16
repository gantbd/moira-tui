//! Stable read-only reattach challenge for one detached governed session.

use std::{error::Error, io};

use moira_client::AgentParticipationReadOnlyClient;
use moira_protocol::agent_participation as participation;
use moira_tui_local_client::LocalAgentParticipationPort;
use serde_json::json;

pub fn run(
    port: LocalAgentParticipationPort,
    id: participation::ParticipationIdV1,
) -> Result<(), Box<dyn Error>> {
    let mut client = AgentParticipationReadOnlyClient::new(port);
    let first = current(&mut client, id)?;
    let confirmation = challenge(&mut client, id)?;
    if participation::reattach_confirmation(confirmation.storage_revision, &first)
        != Some(confirmation)
    {
        return emit_status("inconsistent");
    }
    let second = current(&mut client, id)?;
    let second_challenge = challenge(&mut client, id)?;
    if first != second || confirmation != second_challenge {
        return emit_status("stale");
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": 1,
            "capability": "agent.reattach.confirmation-view.v1",
            "status": "current",
            "participation": hex(&first.binding.participation.0),
            "session": hex(&first.binding.session.0),
            "storage_revision": confirmation.storage_revision,
            "participation_revision": confirmation.participation_revision,
            "session_revision": confirmation.session_revision,
            "method": {
                "machine_digest": hex(&first.binding.method.machine_digest),
                "state_digest": hex(&first.binding.method.state_digest),
            },
            "grounding": {
                "authority_digest": hex(&first.binding.grounding.authority_digest),
                "surface_digest": hex(&first.binding.grounding.surface_digest),
                "depth": format!("{:?}", first.binding.grounding.depth),
            },
            "persona": format!("{:?}", first.binding.persona),
            "confirmation_digest": hex(&confirmation.confirmation_digest),
        }))?
    );
    Ok(())
}

fn current(
    client: &mut AgentParticipationReadOnlyClient<LocalAgentParticipationPort>,
    id: participation::ParticipationIdV1,
) -> Result<participation::ParticipationViewV1, Box<dyn Error>> {
    match client.participation(id).map_err(transport)? {
        participation::ReplyV1::Accepted(value) => match *value {
            participation::OutcomeV1::Participation(Some(value)) => Ok(value),
            participation::OutcomeV1::Participation(None) => {
                Err(io::Error::new(io::ErrorKind::NotFound, "participation not found").into())
            }
            _ => Err(io::Error::other("invalid participation reply").into()),
        },
        participation::ReplyV1::Refused(_) => Err(io::Error::other("participation refused").into()),
    }
}

fn challenge(
    client: &mut AgentParticipationReadOnlyClient<LocalAgentParticipationPort>,
    id: participation::ParticipationIdV1,
) -> Result<participation::ReattachConfirmationV1, Box<dyn Error>> {
    match client.reattach_confirmation(id).map_err(transport)? {
        participation::ReplyV1::Accepted(value) => match *value {
            participation::OutcomeV1::ReattachConfirmation(Some(value)) => Ok(value),
            participation::OutcomeV1::ReattachConfirmation(None) => {
                Err(io::Error::other("session does not require reattach").into())
            }
            _ => Err(io::Error::other("invalid reattach reply").into()),
        },
        participation::ReplyV1::Refused(_) => Err(io::Error::other("reattach view refused").into()),
    }
}

fn emit_status(status: &str) -> Result<(), Box<dyn Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": 1,
            "capability": "agent.reattach.confirmation-view.v1",
            "status": status,
        }))?
    );
    Ok(())
}

fn transport(error: moira_tui_local_client::ClientError) -> io::Error {
    io::Error::other(format!("{error:?}"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

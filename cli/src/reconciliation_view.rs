//! Read-only presentation of a server-prepared saga reconciliation challenge.

use std::{error::Error, io};

use moira_client::unix::{SagaClientErrorV1 as ClientError, SagaUnixPort};
use moira_protocol::ReconciliationProposalV1;
use moira_reconciliation_command::{PREPARE_CAPABILITY, PrepareError, prepare};
use serde_json::json;

pub fn run(port: SagaUnixPort, proposal: ReconciliationProposalV1) -> Result<(), Box<dyn Error>> {
    match prepare(port, proposal) {
        Ok(value) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": 1,
                    "capability": PREPARE_CAPABILITY,
                    "status": "current",
                    "storage_revision": value.storage_revision,
                    "saga": value.saga.0,
                    "disposition": format!("{:?}", value.disposition),
                    "action": hex(&value.action.0),
                    "confirmation_digest": hex(&value.confirmation_digest),
                }))?
            );
            Ok(())
        }
        Err(PrepareError::NotAvailable) => {
            fail("not_available", "no current reconciliation challenge")
        }
        Err(PrepareError::Refused(refusal)) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": 1,
                    "capability": PREPARE_CAPABILITY,
                    "status": "refused",
                    "refusal": format!("{refusal:?}"),
                }))?
            );
            Err(io::Error::other("reconciliation confirmation refused").into())
        }
        Err(PrepareError::InvalidReply) => fail(
            "invalid_reply",
            "unexpected reconciliation confirmation reply",
        ),
        Err(PrepareError::Transport(ClientError::NotSent(kind))) => {
            Err(io::Error::new(kind, "not sent").into())
        }
        Err(PrepareError::Transport(ClientError::OutcomeUnknown(kind))) => {
            Err(io::Error::new(kind, "read outcome unknown").into())
        }
        Err(PrepareError::Transport(_)) => {
            fail("invalid_reply", "invalid reconciliation confirmation reply")
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|value| format!("{value:02x}")).collect()
}

fn fail(status: &str, message: &str) -> Result<(), Box<dyn Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": 1,
            "capability": PREPARE_CAPABILITY,
            "status": status,
        }))?
    );
    Err(io::Error::other(message).into())
}

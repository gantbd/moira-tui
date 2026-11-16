//! Explicit submission of one exact server-prepared saga reconciliation.

use std::{error::Error, io};

use moira_client::unix::{SagaClientErrorV1 as ClientError, SagaUnixPort};
use moira_protocol::{ConfirmedReconciliationV1, SagaOutcomeV1};
use moira_reconciliation_command::{CONFIRM_CAPABILITY, ConfirmError, confirm};
use serde_json::json;

pub fn run(
    port: SagaUnixPort,
    confirmation: ConfirmedReconciliationV1,
) -> Result<(), Box<dyn Error>> {
    let action = hex(&confirmation.action.0);
    match confirm(port, confirmation) {
        Ok(SagaOutcomeV1::Applied {
            storage_revision,
            transition,
            replayed,
        }) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": 1,
                    "capability": CONFIRM_CAPABILITY,
                    "status": "accepted",
                    "storage_revision": storage_revision,
                    "transition": format!("{transition:?}"),
                    "action": action,
                    "replayed": replayed,
                }))?
            );
            Ok(())
        }
        Err(ConfirmError::Refused(refusal)) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": 1,
                    "capability": CONFIRM_CAPABILITY,
                    "status": "refused",
                    "refusal": format!("{refusal:?}"),
                    "action": action,
                }))?
            );
            Err(io::Error::other("reconciliation refused").into())
        }
        Ok(_) | Err(ConfirmError::InvalidReply) => {
            fail("invalid_reply", &action, "unexpected reconciliation reply")
        }
        Err(ConfirmError::Transport(ClientError::NotSent(kind))) => {
            transport("not_sent", kind, true, &action)
        }
        Err(ConfirmError::Transport(ClientError::OutcomeUnknown(kind))) => {
            transport("outcome_unknown", kind, false, &action)
        }
        Err(ConfirmError::Transport(ClientError::RequestLimit)) => {
            fail("request_limit", &action, "request limit reached")
        }
        Err(ConfirmError::Transport(
            ClientError::InvalidLimits | ClientError::InvalidReady | ClientError::InvalidReply,
        )) => fail("invalid_reply", &action, "invalid reconciliation reply"),
    }
}

fn transport(
    status: &str,
    kind: io::ErrorKind,
    retry_safe: bool,
    action: &str,
) -> Result<(), Box<dyn Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": 1,
            "capability": CONFIRM_CAPABILITY,
            "status": status,
            "retry_safe": retry_safe,
            "action": action,
            "recovery": if retry_safe { "resubmit exact confirmation" } else { "inspect the operator receipt for action before any further reconciliation" },
        }))?
    );
    Err(io::Error::new(kind, status).into())
}

fn fail(status: &str, action: &str, message: &str) -> Result<(), Box<dyn Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": 1,
            "capability": CONFIRM_CAPABILITY,
            "status": status,
            "action": action,
        }))?
    );
    Err(io::Error::other(message).into())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|value| format!("{value:02x}")).collect()
}

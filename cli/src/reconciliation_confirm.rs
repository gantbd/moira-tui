//! Explicit submission of one exact server-prepared saga reconciliation.

use std::{error::Error, io};

use moira_client::SagaConfirmedReconciliationClient;
use moira_protocol::{ConfirmedReconciliationV1, SagaOutcomeV1, SagaReplyV1};
use moira_tui_local_client::{ClientError, LocalSagaPort};
use serde_json::json;

pub fn run(
    port: LocalSagaPort,
    confirmation: ConfirmedReconciliationV1,
) -> Result<(), Box<dyn Error>> {
    let action = hex(&confirmation.action.0);
    let mut client = SagaConfirmedReconciliationClient::new(port);
    match client.confirm(confirmation) {
        Ok(SagaReplyV1::Accepted(SagaOutcomeV1::Applied {
            storage_revision,
            transition,
            replayed,
        })) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": 1,
                    "capability": "sagas.reconciliation.confirm.v1",
                    "status": "accepted",
                    "storage_revision": storage_revision,
                    "transition": format!("{transition:?}"),
                    "action": action,
                    "replayed": replayed,
                }))?
            );
            Ok(())
        }
        Ok(SagaReplyV1::Refused(refusal)) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": 1,
                    "capability": "sagas.reconciliation.confirm.v1",
                    "status": "refused",
                    "refusal": format!("{refusal:?}"),
                    "action": action,
                }))?
            );
            Err(io::Error::other("reconciliation refused").into())
        }
        Ok(_) => fail("invalid_reply", &action, "unexpected reconciliation reply"),
        Err(ClientError::NotSent(kind)) => transport("not_sent", kind, true, &action),
        Err(ClientError::OutcomeUnknown(kind)) => {
            transport("outcome_unknown", kind, false, &action)
        }
        Err(ClientError::RequestLimit) => fail("request_limit", &action, "request limit reached"),
        Err(ClientError::InvalidLimits | ClientError::InvalidReady | ClientError::InvalidReply) => {
            fail("invalid_reply", &action, "invalid reconciliation reply")
        }
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
            "capability": "sagas.reconciliation.confirm.v1",
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
            "capability": "sagas.reconciliation.confirm.v1",
            "status": status,
            "action": action,
        }))?
    );
    Err(io::Error::other(message).into())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|value| format!("{value:02x}")).collect()
}

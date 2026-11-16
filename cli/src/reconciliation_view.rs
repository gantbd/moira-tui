//! Read-only presentation of a server-prepared saga reconciliation challenge.

use std::{error::Error, io};

use moira_client::SagaReadOnlyClient;
use moira_protocol::{ReconciliationProposalV1, SagaOutcomeV1, SagaReplyV1};
use moira_tui_local_client::{ClientError, LocalSagaPort};
use serde_json::json;

pub fn run(port: LocalSagaPort, proposal: ReconciliationProposalV1) -> Result<(), Box<dyn Error>> {
    let mut client = SagaReadOnlyClient::new(port);
    match client.reconciliation_confirmation(proposal) {
        Ok(SagaReplyV1::Accepted(SagaOutcomeV1::ReconciliationConfirmation(Some(value)))) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": 1,
                    "capability": "sagas.reconciliation.confirmation.v1",
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
        Ok(SagaReplyV1::Accepted(SagaOutcomeV1::ReconciliationConfirmation(None))) => {
            fail("not_available", "no current reconciliation challenge")
        }
        Ok(SagaReplyV1::Refused(refusal)) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": 1,
                    "capability": "sagas.reconciliation.confirmation.v1",
                    "status": "refused",
                    "refusal": format!("{refusal:?}"),
                }))?
            );
            Err(io::Error::other("reconciliation confirmation refused").into())
        }
        Ok(_) => fail(
            "invalid_reply",
            "unexpected reconciliation confirmation reply",
        ),
        Err(ClientError::NotSent(kind)) => Err(io::Error::new(kind, "not sent").into()),
        Err(ClientError::OutcomeUnknown(kind)) => {
            Err(io::Error::new(kind, "read outcome unknown").into())
        }
        Err(_) => fail("invalid_reply", "invalid reconciliation confirmation reply"),
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
            "capability": "sagas.reconciliation.confirmation.v1",
            "status": status,
        }))?
    );
    Err(io::Error::other(message).into())
}

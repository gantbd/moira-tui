//! Read-only recovery of an indeterminate reconciliation from its durable receipt.

use std::{error::Error, io};

use moira_client::unix::SagaClientErrorV1 as ClientError;
use moira_protocol::{OperatorActionIdV1, SagaIdV1};
use moira_reconciliation_command::{RECOVER_CAPABILITY, RecoverError, recover};
use serde_json::json;

pub fn run(
    port: moira_client::unix::SagaUnixPort,
    saga: SagaIdV1,
    action: OperatorActionIdV1,
) -> Result<(), Box<dyn Error>> {
    match recover(port, saga, action) {
        Ok(receipt) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": 1,
                    "capability": RECOVER_CAPABILITY,
                    "status": "recovered",
                    "action": hex(&receipt.action.0),
                    "saga": receipt.saga.0,
                    "authority_decision": receipt.authority_decision,
                    "authority_epoch": receipt.authority_epoch,
                    "disposition": format!("{:?}", receipt.disposition),
                    "phase": format!("{:?}", receipt.phase),
                    "prepared_effect": receipt.prepared.map(|effect| effect.0),
                }))?
            );
            Ok(())
        }
        Err(RecoverError::NotFound) => fail("not_found", "durable operator receipt not found"),
        Err(RecoverError::Mismatch) => fail("mismatch", "operator receipt identity mismatch"),
        Err(RecoverError::Refused(refusal)) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": 1,
                    "capability": RECOVER_CAPABILITY,
                    "status": "refused",
                    "refusal": format!("{refusal:?}"),
                }))?
            );
            Err(io::Error::other("operator receipt recovery refused").into())
        }
        Err(RecoverError::Transport(ClientError::NotSent(kind))) => {
            Err(io::Error::new(kind, "recovery request not sent").into())
        }
        Err(RecoverError::Transport(ClientError::OutcomeUnknown(kind))) => {
            Err(io::Error::new(kind, "read recovery outcome unknown").into())
        }
        Err(RecoverError::Transport(_) | RecoverError::InvalidReply) => {
            fail("invalid_reply", "invalid operator receipt reply")
        }
    }
}

fn fail(status: &str, message: &str) -> Result<(), Box<dyn Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": 1,
            "capability": RECOVER_CAPABILITY,
            "status": status,
        }))?
    );
    Err(io::Error::other(message).into())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

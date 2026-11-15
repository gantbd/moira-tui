#![cfg(unix)]
#![forbid(unsafe_code)]

use std::{convert::Infallible, os::unix::net::UnixStream, path::PathBuf, time::Duration};

use moira_core::saga::{SagaLimitsV1, persistence::SagaCodecLimitsV1};
use moira_local_daemon::unix::{SagaAuthenticator, SagaDaemon};
use moira_protocol::{
    OperatorActionIdV1, OperatorDispositionV1, OperatorOperationV1, SagaCommandV1,
    SagaEffectOutcomeV1, SagaMutationV1, SagaOutcomeV1, SagaReplyV1, SagaSourceV1,
    SagaTransitionV1,
};
use moira_service::{
    AccessV1,
    saga::{OperatorAccessV1, SagaService},
};
use moira_sqlite::saga::{SagaSqliteStore, SagaStoreLimitsV1};
use moira_wire::LimitsV1;

struct ReadOnlyAuthentication;

impl SagaAuthenticator for ReadOnlyAuthentication {
    type Error = Infallible;

    fn authenticate(&mut self, _: &UnixStream) -> Result<AccessV1, Self::Error> {
        Ok(AccessV1 {
            read: true,
            mutate: false,
        })
    }

    fn authorize_operator(
        &mut self,
        _: OperatorOperationV1,
    ) -> Result<Option<OperatorAccessV1>, Self::Error> {
        Ok(None)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let database = args.next().ok_or("database path required")?;
    let socket = args.next().ok_or("socket path required")?;
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let limits = SagaStoreLimitsV1 {
        codec: SagaCodecLimitsV1 {
            max_bytes: 64 * 1024,
            max_sagas: 128,
            max_operator_actions: 128,
        },
        saga: SagaLimitsV1 {
            max_sagas: 128,
            max_retries_per_saga: 8,
            max_inspection_page: 64,
            max_operator_actions: 128,
        },
        max_database_pages: 256,
    };
    let store = SagaSqliteStore::open(database, [8; 16], limits)?;
    let mut service = SagaService::new(store);
    let seeded = service.execute(SagaCommandV1::Apply {
        expected_revision: 0,
        mutation: SagaMutationV1::Admit {
            source: SagaSourceV1([2; 16]),
            now: 10,
            deadline: 20,
            retry_budget: 2,
        },
    })?;
    eprintln!("seeded {seeded:?}");
    let SagaReplyV1::Accepted(SagaOutcomeV1::Applied {
        transition: SagaTransitionV1::Admitted { saga, effect, .. },
        ..
    }) = seeded
    else {
        return Err("admission seed returned an unexpected reply".into());
    };
    let evidence = service.execute(SagaCommandV1::Apply {
        expected_revision: 1,
        mutation: SagaMutationV1::AcceptEffect {
            saga,
            effect,
            outcome: SagaEffectOutcomeV1::Uncertain,
            observed_at: 11,
        },
    })?;
    let SagaReplyV1::Accepted(SagaOutcomeV1::Applied { .. }) = evidence else {
        return Err("evidence seed returned an unexpected reply".into());
    };
    let reconciled = service.execute_operator_authorized(
        OperatorAccessV1 {
            subject: [3; 16],
            authority_decision: 9,
            authority_epoch: 10,
        },
        SagaCommandV1::Reconcile {
            expected_revision: 2,
            operation: OperatorOperationV1 {
                action: OperatorActionIdV1([4; 16]),
                saga,
                disposition: OperatorDispositionV1::KeepUncertain,
            },
        },
    )?;
    eprintln!("reconciled {reconciled:?}");
    let mut daemon = SagaDaemon::bind_new(
        socket,
        service,
        ReadOnlyAuthentication,
        LimitsV1::default(),
        Duration::from_secs(5),
        8,
    )?;
    daemon
        .serve_next()
        .map_err(|error| std::io::Error::other(format!("daemon failed: {error:?}")))?;
    Ok(())
}

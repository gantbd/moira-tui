#[test]
fn reusable_component_has_no_cli_or_terminal_owner() {
    let source = include_str!("../src/lib.rs");
    assert!(!source.contains("std::env::args"));
    assert!(!source.contains("ratatui::run"));
    assert!(!source.contains("process::exit"));
    assert!(!source.contains("SagaCommandV1::Reconcile"));
    assert!(!source.contains("SagaOperatorClient"));
    assert!(source.contains(".evidence("));
    assert!(source.contains(".operator_receipts("));
}

#[test]
fn batch_reconciliation_preserves_independent_owner_guards_and_receipts() {
    let source = include_str!("../src/reconciliation_batch.rs");
    for required in [
        "MAX_RECONCILIATION_BATCH",
        "stable_snapshot(&mut reader, proposal, None)",
        "operator_receipt(action)",
        "ChildState::NotSent",
        "Confirmation::OutcomeUnknown",
        "break;",
    ] {
        assert!(source.contains(required), "batch omits {required}");
    }
    for forbidden in ["SagaOperatorClient", "SagaCommandV1::Reconcile", "retry("] {
        assert!(!source.contains(forbidden), "batch imports {forbidden}");
    }
}

#[test]
fn capability_manifest_advertises_every_headless_governed_control_schema() {
    let headless = moira_tui::CAPABILITIES.headless;
    for capability in [
        "sagas.reconciliation.confirmation.v1",
        "sagas.reconciliation.confirm.v1",
        "agent.prompt.submit.v1",
        "agent.permission.confirmation-view.v1",
        "agent.permission.confirm.v1",
        "agent.cancel.confirmation-view.v1",
        "agent.cancel.confirm.v1",
        "agent.reattach.confirmation-view.v1",
        "agent.reattach.confirm.v1",
    ] {
        assert!(
            headless.contains(&capability),
            "standalone schema {capability} is not discoverable"
        );
    }
}

#[test]
fn reattach_control_carries_only_server_confirmation_and_no_execution_material() {
    let view = include_str!("../cli/src/reattach_view.rs");
    let confirm = include_str!("../cli/src/reattach_confirm.rs");
    let interactive = include_str!("../src/reattach_control.rs");
    assert!(view.contains("AgentParticipationReadOnlyClient"));
    assert!(view.contains("reattach_confirmation"));
    assert!(confirm.contains("AgentParticipationReattachClient"));
    assert!(confirm.contains("ConfirmedReattachV1"));
    assert!(interactive.contains("ConfirmedReattachV1"));
    for source in [view, confirm, interactive] {
        assert!(!source.contains("ExecutionV1"));
        assert!(!source.contains("ExecutionIdV1"));
        assert!(!source.contains("IdempotencyKeyV1"));
        assert!(!source.contains("AttemptV1"));
        assert!(!source.contains("daimon"));
        assert!(!source.contains("aegis"));
    }
}

#[test]
fn open_control_uses_only_the_owner_derived_preparation_client() {
    let interactive = include_str!("../src/open_control.rs");
    assert!(interactive.contains("AgentParticipationOpenClient"));
    assert!(interactive.contains("PrepareAgentOpenDispositionV1"));
    assert!(!interactive.contains("AgentParticipationMutationClient"));
    assert!(!interactive.contains("IdempotencyKeyV1"));
    assert!(!interactive.contains("ExecutionIdV1"));
    assert!(!interactive.contains("daimon"));
    assert!(!interactive.contains("aegis"));
}

#[test]
fn headless_cli_uses_public_read_only_client_without_tui_state() {
    let source = include_str!("../cli/src/main.rs");
    let headless = source.split("fn run_headless").nth(1).unwrap();
    assert!(headless.contains("SagaReadOnlyClient"));
    assert!(headless.contains("client.census()"));
    assert!(!headless.contains("Component::new"));
}

#[test]
fn agent_observation_uses_moira_public_client_and_never_daimon() {
    let source = include_str!("../cli/src/main.rs");
    let agent = source.split("fn run_agent_headless").nth(1).unwrap();
    assert!(agent.contains("AgentReadOnlyClient"));
    assert!(!agent.contains("daimon"));
    assert!(!agent.contains("AgentMutationClient"));
}

#[test]
fn participation_observation_exposes_no_mutation_or_authority_client() {
    let source = include_str!("../cli/src/main.rs");
    let participation = source
        .split("fn run_participation_headless")
        .nth(1)
        .unwrap()
        .split("fn operation_kind")
        .next()
        .unwrap();
    assert!(participation.contains("AgentParticipationReadOnlyClient"));
    assert!(!participation.contains("AgentParticipationMutationClient"));
    assert!(!participation.contains("AgentParticipationPermissionClient"));
    assert!(!participation.contains("daimon"));
}

#[test]
fn permission_confirmation_view_correlates_only_public_read_models() {
    let source = include_str!("../cli/src/permission_view.rs");
    assert!(source.contains("AgentParticipationReadOnlyClient"));
    assert!(source.contains("AgentReadOnlyClient"));
    assert!(source.contains("permission_confirmation"));
    assert!(!source.contains("AgentParticipationPermissionClient"));
    assert!(!source.contains("AgentParticipationMutationClient"));
    assert!(!source.contains("AgentMutationClient"));
    assert!(!source.contains("daimon"));
}

#[test]
fn permission_confirmation_mutation_carries_no_authority_or_execution_material() {
    let source = include_str!("../cli/src/permission_confirm.rs");
    assert!(source.contains("AgentParticipationPermissionClient"));
    assert!(source.contains("ConfirmedPermissionSelectionV1"));
    assert!(!source.contains("participation::PermissionSelectionV1"));
    assert!(!source.contains("PermissionAccessV1"));
    assert!(!source.contains("ExecutionIdV1"));
    assert!(!source.contains("IdempotencyKeyV1"));
    assert!(!source.contains("aegis"));
    assert!(!source.contains("daimon"));
}

#[test]
fn saga_reconciliation_uses_only_the_server_prepared_confirmation() {
    let view = include_str!("../cli/src/reconciliation_view.rs");
    let confirm = include_str!("../cli/src/reconciliation_confirm.rs");
    let recover = include_str!("../cli/src/reconciliation_recover.rs");
    let guard = include_str!("../../moira/crates/moira-reconciliation-command/src/lib.rs");
    assert!(view.contains("moira_reconciliation_command"));
    assert!(view.contains("prepare"));
    assert!(confirm.contains("moira_reconciliation_command"));
    assert!(confirm.contains("confirm"));
    assert!(confirm.contains("ConfirmedReconciliationV1"));
    assert!(recover.contains("recover"));
    assert!(guard.contains("SagaReadOnlyClient"));
    assert!(guard.contains("reconciliation_confirmation"));
    assert!(guard.contains("SagaConfirmedReconciliationClient"));
    assert!(guard.contains("operator_receipt"));
    for source in [view, confirm, recover, guard] {
        assert!(!source.contains("SagaOperatorClient"));
        assert!(!source.contains("OperatorOperationV1"));
        assert!(!source.contains("OperatorAccessV1"));
        assert!(!source.contains("SagaCommandV1::Reconcile"));
        assert!(!source.contains("aegis"));
        assert!(!source.contains("daimon"));
    }
}

#[test]
fn interactive_reconciliation_recovers_unknown_outcomes_by_exact_receipt() {
    let source = include_str!("../src/reconciliation_control.rs");
    assert!(source.contains("SagaReadOnlyClient"));
    assert!(source.contains("SagaConfirmedReconciliationClient"));
    assert!(source.contains("ConfirmedReconciliationV1"));
    assert!(source.contains("operator_receipt(action)"));
    assert!(source.contains("fenced_action"));
    for forbidden in [
        "SagaOperatorClient",
        "OperatorOperationV1",
        "OperatorAccessV1",
        "SagaCommandV1::Reconcile",
        "aegis",
        "daimon",
    ] {
        assert!(!source.contains(forbidden), "found {forbidden}");
    }
}

#[test]
fn cancellation_view_and_confirmation_carry_no_provider_or_execution_material() {
    let view = include_str!("../cli/src/cancel_view.rs");
    let confirm = include_str!("../cli/src/cancel_confirm.rs");
    assert!(view.contains("AgentParticipationReadOnlyClient"));
    assert!(view.contains("cancel_confirmation"));
    assert!(confirm.contains("AgentParticipationCancellationClient"));
    assert!(confirm.contains("ConfirmedCancelV1"));
    for source in [view, confirm] {
        assert!(!source.contains("AgentMutationClient"));
        assert!(!source.contains("ExecutionIdV1"));
        assert!(!source.contains("IdempotencyKeyV1"));
        assert!(!source.contains("aegis"));
        assert!(!source.contains("daimon"));
    }
}

#[test]
fn prompt_submission_carries_bytes_but_no_operation_or_authority_material() {
    let source = include_str!("../cli/src/prompt_submit.rs");
    assert!(source.contains("AgentParticipationPromptClient"));
    assert!(source.contains("MAX_PROMPT_INPUT_BYTES"));
    assert!(source.contains("read_to_end"));
    for forbidden in [
        "AgentParticipationMutationClient",
        "AgentMutationClient",
        "ExecutionIdV1",
        "IdempotencyKeyV1",
        "AgentTurnIdV1",
        "AttemptV1",
        "PermissionAccessV1",
        "Sha256",
        "aegis",
        "daimon",
    ] {
        assert!(!source.contains(forbidden), "found {forbidden}");
    }
}

#[test]
fn terminal_peer_has_no_prompt_materialization_capability() {
    for source in [
        include_str!("../src/lib.rs"),
        include_str!("../src/participation.rs"),
        include_str!("../src/prompt_control.rs"),
        include_str!("../src/permission_control.rs"),
        include_str!("../src/cancel_control.rs"),
        include_str!("../cli/src/main.rs"),
        include_str!("../cli/src/prompt_submit.rs"),
        include_str!("../local-client/src/lib.rs"),
    ] {
        assert!(!source.contains("AgentParticipationMaterializationClient"));
        assert!(!source.contains("MaterializePrompt"));
        assert!(!source.contains("agent.prompt.materialize"));
    }
}

#[test]
fn interactive_participation_component_is_read_only_and_provider_neutral() {
    let source = include_str!("../src/participation.rs");
    assert!(source.contains("AgentParticipationReadOnlyClient"));
    assert!(source.contains("AgentParticipationServicePort"));
    assert!(!source.contains("AgentParticipationMutationClient"));
    assert!(!source.contains("AgentParticipationPromptClient"));
    assert!(!source.contains("AgentParticipationPermissionClient"));
    assert!(!source.contains("AgentParticipationCancellationClient"));
    assert!(!source.contains("daimon"));
    assert!(!source.contains("provider"));
}

#[test]
fn interactive_agent_component_uses_only_the_public_read_model() {
    let source = include_str!("../src/agent.rs");
    assert!(source.contains("AgentReadOnlyClient"));
    assert!(source.contains("AgentServicePort"));
    assert!(source.contains("read_session(client.inspect"));
    assert!(source.contains("client.events"));
    assert!(!source.contains("AgentMutationClient"));
    assert!(!source.contains("AgentParticipationMutationClient"));
    assert!(!source.contains("daimon"));
    assert!(!source.contains("provider.invoke"));
}

#[test]
fn deliberation_views_use_only_the_stable_public_read_model() {
    let component = include_str!("../src/deliberation.rs");
    let headless = include_str!("../cli/src/deliberation_view.rs");
    for source in [component, headless] {
        assert!(source.contains("DeliberationReadOnlyClient"));
        assert!(source.contains("observe_deliberation"));
        assert!(!source.contains("DeliberationMutationClient"));
        assert!(!source.contains("pareto_frontier"));
        assert!(!source.contains("moira_core"));
        assert!(!source.contains("daimon"));
        assert!(!source.contains("aegis"));
    }
    assert!(component.matches("client.status()").count() >= 2);
    assert!(headless.contains("dissent_digests"));
    assert!(headless.contains("authorized_choice"));
    assert!(headless.contains("\"mu\""));
}

#[test]
fn interactive_cancellation_uses_separate_capability_clients_and_no_invented_identity() {
    let source = include_str!("../src/cancel_control.rs");
    assert!(source.contains("AgentParticipationReadOnlyClient"));
    assert!(source.contains("AgentParticipationCancellationClient"));
    assert!(source.contains("ConfirmedCancelV1"));
    assert!(source.contains("cancel_confirmation"));
    for forbidden in [
        "AgentParticipationMutationClient",
        "AgentMutationClient",
        "ExecutionIdV1",
        "IdempotencyKeyV1",
        "PermissionAccessV1",
        "aegis",
        "daimon",
        "provider",
    ] {
        assert!(!source.contains(forbidden), "found {forbidden}");
    }
}

#[test]
fn interactive_permission_correlates_exact_events_and_uses_only_confirmed_selection() {
    let source = include_str!("../src/permission_control.rs");
    assert!(source.contains("AgentParticipationReadOnlyClient"));
    assert!(source.contains("AgentReadOnlyClient"));
    assert!(source.contains("AgentParticipationPermissionClient"));
    assert!(source.contains("ConfirmedPermissionSelectionV1"));
    assert!(source.contains("permission_confirmation"));
    assert!(source.contains("complete_events"));
    for forbidden in [
        "AgentParticipationMutationClient",
        "AgentMutationClient",
        "ExecutionIdV1",
        "IdempotencyKeyV1",
        "PermissionAccessV1",
        "aegis",
        "daimon",
        "provider",
    ] {
        assert!(!source.contains(forbidden), "found {forbidden}");
    }
}

#[test]
fn interactive_prompt_carries_only_exact_bounded_bytes_and_fences_uncertainty() {
    let source = include_str!("../src/prompt_control.rs");
    assert!(source.contains("AgentParticipationReadOnlyClient"));
    assert!(source.contains("AgentParticipationPromptClient"));
    assert!(source.contains("MAX_INTERACTIVE_PROMPT_BYTES"));
    assert!(source.contains("OutcomeUnknown"));
    for forbidden in [
        "AgentParticipationMutationClient",
        "AgentMutationClient",
        "ExecutionIdV1",
        "IdempotencyKeyV1",
        "AgentTurnIdV1",
        "AttemptV1",
        "PermissionAccessV1",
        "aegis",
        "daimon",
        "provider",
    ] {
        assert!(!source.contains(forbidden), "found {forbidden}");
    }
}

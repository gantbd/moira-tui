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

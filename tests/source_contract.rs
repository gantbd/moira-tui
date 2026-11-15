#[test]
fn reusable_component_has_no_cli_or_terminal_owner() {
    let source = include_str!("../src/lib.rs");
    assert!(!source.contains("std::env::args"));
    assert!(!source.contains("ratatui::run"));
    assert!(!source.contains("process::exit"));
    assert!(!source.contains("SagaCommandV1::Reconcile"));
}

#[test]
fn headless_cli_uses_public_read_only_client_without_tui_state() {
    let source = include_str!("../cli/src/main.rs");
    let headless = source.split("fn run_headless").nth(1).unwrap();
    assert!(headless.contains("SagaReadOnlyClient"));
    assert!(!headless.contains("Component::new"));
}

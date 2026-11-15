# Moira TUI

Composable read-only terminal presentation for Moira's public saga client.

The root `moira-tui` library owns bounded workflow inspection, aggregate census,
selection, reconciliation-state, effect-evidence, and operator-receipt
presentation, closed input, and lifecycle outputs. It owns no terminal setup,
argument parsing, daemon startup, process exit, workflow mutation, operator
authorization, or reconciliation decisions.

The separate `moira-tui-cli` package provides the standalone executable:

```text
moira-tui capabilities
moira-tui interactive /path/to/moira.sock
moira-tui headless /path/to/moira.sock
```

Headless mode uses `moira-client::SagaReadOnlyClient` directly and emits a
versioned `sagas.inspect.v1` JSON observation, including the same content-free
census, without constructing presentation state. Interactive mode hosts the
reusable component over the same private Unix service profile.

Opening a selected saga reads bounded evidence and the bounded operator-receipt
page through `SagaReadOnlyClient`. Receipt disposition and authority decision
metadata are evidence only; the component neither constructs nor submits a
reconciliation operation.

The current source dependency points at the actively developed sibling `moira`
worktree. A reproducible source receipt must wait until that boundary is committed.

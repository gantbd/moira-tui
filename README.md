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
moira-tui agent-inspect /path/to/moira-agent.sock SESSION_HEX
moira-tui agent-events /path/to/moira-agent.sock SESSION_HEX [AFTER|-]
moira-tui participation-inspect /path/to/moira-participation.sock PARTICIPATION_HEX
moira-tui permission-view /path/to/moira-participation.sock /path/to/moira-agent.sock PARTICIPATION_HEX
moira-tui permission-confirm /path/to/moira-participation.sock PARTICIPATION_HEX OPTION_HEX CONFIRMATION_DIGEST_HEX CONFIRM
moira-tui cancel-view /path/to/moira-participation.sock PARTICIPATION_HEX
moira-tui cancel-confirm /path/to/moira-participation.sock PARTICIPATION_HEX CONFIRMATION_DIGEST_HEX CONFIRM
```

Headless mode uses `moira-client::SagaReadOnlyClient` directly and emits a
versioned `sagas.inspect.v1` JSON observation, including the same content-free
census, without constructing presentation state. Interactive mode hosts the
reusable component over the same private Unix service profile.

The session and event commands are read-only, versioned projections over
Moira's separate public agent service and canonical wire. Event byte strings are
emitted as exact JSON byte arrays rather than being silently reinterpreted as
UTF-8.
The CLI neither imports Daimon nor connects to an ACP/provider process.
`participation-inspect` reads the durable governed record, including its exact
Rune method, Dendrite grounding digests, persona, phase, and pending-operation
kind. It exposes no mutation or permission-authority injection surface.
`permission-view` joins that stable record to a complete, cursor-contiguous
agent-event observation, verifies the exact retained request, turn, capability
digest, option digest and option identities, and emits byte-preserving labels
plus a domain-separated confirmation fingerprint. It rereads both services and
reports stale or inconsistent evidence instead of presenting a mixed snapshot.
The fingerprint is presentation evidence, not permission authority.
`permission-confirm` submits only the participation, selected option, exact
server-prepared fingerprint, and an explicit `CONFIRM` marker. Moira revalidates
the current durable record, derives replay-stable execution and idempotency
identities, then obtains command-scoped Aegis authorization. The terminal never
constructs or receives authority credentials. A transport-unknown result is
reported as such and directs the operator to inspect participation before any
further action.
`cancel-view` and `cancel-confirm` apply the same current-revision discipline to
an active turn. Moira binds the challenge to the exact attempt, method,
grounding, persona, session revision and turn, derives the cancel key, and
requires authenticated mutation access. The terminal never supplies a turn or
idempotency identity.

The end-to-end qualification tests start the real bounded Moira participation
and agent Unix profiles. They prove view-to-confirm permission selection with
one Aegis authorization call and view-to-confirm cancellation with no invented
turn/key or permission-authority call; both finish as exact durable pending
operations.

Opening a selected saga reads bounded evidence and the bounded operator-receipt
page through `SagaReadOnlyClient`. Receipt disposition and authority decision
metadata are evidence only; the component neither constructs nor submits a
reconciliation operation.

The source dependency points at Moira's committed public client/protocol
boundary. Workbench records the exact Moira and Moira TUI revisions separately;
the reusable presentation still imports no coordinator, storage, daemon, peer,
or authority implementation.

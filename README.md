# Moira TUI

Composable terminal presentation and closed governed controls for Moira's
public clients.

The root `moira-tui` library owns bounded workflow inspection, aggregate census,
selection, reconciliation-state, effect-evidence, and operator-receipt
presentation, closed input, and lifecycle outputs. It owns no terminal setup,
argument parsing, daemon startup, process exit, workflow mutation, operator
authorization, or reconciliation decisions.

`reconciliation-view` requests an exact server-prepared challenge for one
operator disposition. `reconciliation-confirm` returns that challenge's
revision, digest, and server-derived action unchanged behind a literal
`CONFIRM` gate. A lost reply is reported as outcome-unknown with the exact
operator-receipt action to inspect; the CLI does not retry or infer success.
`reconciliation-interactive` hosts the reusable arm/confirm component over the
same connection. An unknown confirmation outcome fences the control until
refresh finds the exact durable operator receipt or proves an unchanged current
challenge with no receipt.
The local saga, agent, participation, and deliberation ports reconnect only for
a later explicit command after transport loss; they never retry the uncertain
command. The saga fault-injected socket gate commits the mutation, discards its
reply, reconnects on refresh, and recovers the exact authority-attributed
receipt without a second mutation. A separate participation transport gate
drops a prompt-mutation reply, reports outcome-unknown, and accepts only the
later explicit governed-state read on a fresh connection.

The separate `moira-tui-cli` package provides the standalone executable:

```text
moira-tui capabilities
moira-tui interactive /path/to/moira.sock
moira-tui headless /path/to/moira.sock
moira-tui deliberation-inspect /path/to/moira-deliberation.sock DELIBERATION_HEX
moira-tui deliberation-interactive /path/to/moira-deliberation.sock DELIBERATION_HEX
moira-tui agent-inspect /path/to/moira-agent.sock SESSION_HEX
moira-tui agent-events /path/to/moira-agent.sock SESSION_HEX [AFTER|-]
moira-tui agent-interactive /path/to/moira-agent.sock SESSION_HEX
moira-tui participation-inspect /path/to/moira-participation.sock PARTICIPATION_HEX
moira-tui participation-interactive /path/to/moira-participation.sock PARTICIPATION_HEX
moira-tui prompt-submit /path/to/moira-participation.sock PARTICIPATION_HEX PROMPT_FILE|- CONFIRM
moira-tui prompt-interactive /path/to/moira-participation.sock PARTICIPATION_HEX PROMPT_FILE
moira-tui permission-view /path/to/moira-participation.sock /path/to/moira-agent.sock PARTICIPATION_HEX
moira-tui permission-interactive /path/to/moira-participation.sock /path/to/moira-agent.sock PARTICIPATION_HEX
moira-tui permission-confirm /path/to/moira-participation.sock PARTICIPATION_HEX OPTION_HEX CONFIRMATION_DIGEST_HEX CONFIRM
moira-tui cancel-view /path/to/moira-participation.sock PARTICIPATION_HEX
moira-tui cancel-interactive /path/to/moira-participation.sock PARTICIPATION_HEX
moira-tui cancel-confirm /path/to/moira-participation.sock PARTICIPATION_HEX CONFIRMATION_DIGEST_HEX CONFIRM
moira-tui reattach-view /path/to/moira-participation.sock PARTICIPATION_HEX
moira-tui reattach-interactive /path/to/moira-participation.sock PARTICIPATION_HEX
moira-tui reattach-confirm /path/to/moira-participation.sock PARTICIPATION_HEX CONFIRMATION_DIGEST_HEX CONFIRM
```

`moira-tui capabilities` advertises every versioned headless observation and
governed-control envelope implemented by those commands. Discovery includes the
server-prepared reconciliation, permission, and cancellation view/confirm pairs
plus bounded prompt submission; listing a control is not authority to invoke it.

Headless mode uses `moira-client::SagaReadOnlyClient` directly and emits a
versioned `sagas.inspect.v1` JSON observation, including the same content-free
census, without constructing presentation state. Interactive mode hosts the
reusable component over the same private Unix service profile.

`deliberation-inspect` emits a versioned `deliberation.inspect.v1` observation
over Moira's independent public deliberation client. It preserves exact Rune
method and Dendrite grounding digests, persona lenses, claim kinds, candidate
scores, report evidence and dissent, and the retained proof/authorized-choice/
maybe/mu resolution without recomputing a frontier or treating it as authority.
`deliberation-interactive` hosts the reusable read-only component over the same
bounded private service. Both paths read the registry revision before and after
the deliberation and reject a mixed snapshot; the real-daemon gate reopens the
SQLite owner and qualifies both paths under read-only authentication.

The session and event commands are read-only, versioned projections over
Moira's separate public agent service and canonical wire. Event byte strings are
emitted as exact JSON byte arrays rather than being silently reinterpreted as
UTF-8. A real-daemon `agent-inspect` gate proves that all eight capability bits
reach the versioned JSON envelope unchanged.
`agent-interactive` hosts a reusable read-only component over that same public
service. Each refresh double-reads the session around one bounded event page,
rejects a changing snapshot or noncontiguous cursors, explicitly labels a
truncated page, and renders byte fields as escaped previews rather than assuming
UTF-8. It renders the complete public capability projection, including provider
session load, resume, mode and model support, without turning any advertised
fact into a terminal control. Event selection, bounded cursor-page navigation,
refresh, and close are its only controls. A recovered idle provider binding is rendered as
`ReattachRequired` with an explicit governed-reattach notice. The separate
`reattach-view`/`reattach-confirm` pair and reusable arm/confirm component act
only after the worker has read the exact Daimon session and durably marked the
detached participation. Moira derives the key, execution, and retained attempt
from a domain-separated challenge binding storage/session revisions, Rune
method, Dendrite grounding, persona, profile evidence and cursor. The terminal
receives none of those operation identities, no authority credential and no
provider locator. It never resumes automatically; Daimon still revalidates the
exact retained scope at dispatch.
The restart gate exhausts the first daemon connection, moves
the same gateway owner behind a newly bound daemon, and proves that only an
explicit refresh reconnects and reproduces the exact session/event rendering;
the gateway rejects every mutating command.
The CLI neither imports Daimon nor connects to an ACP/provider process.
`participation-inspect` reads the durable governed record, including its exact
Rune method, Dendrite grounding digests, persona, phase, and pending-operation
kind. It exposes no mutation or permission-authority injection surface.
`participation-interactive` hosts the reusable read-only participation component
over that same public port. It renders phase, revisions, exact attempt counters,
persona, grounding depth, capabilities, pending-operation kind, and retained
permission or terminal disposition. Its capability view includes the complete
session load/resume/mode/model projection rather than only effect-facing facts.
Refresh and close are its only active
controls; it cannot contact a provider or construct a mutation.
`prompt-submit` reads 1 through 61440 bytes from a file or stdin and requires an
explicit `CONFIRM`. It sends only those bytes and the participation. Moira
commits the content before deriving the prompt digest, turn, execution,
idempotency key, and exact retained attempt. The terminal constructs none of
those identities. A transport-unknown result directs the operator to inspect
the participation before taking another action.
`prompt-interactive` accepts the same bound from a file, double-reads the
governed participation, renders an escaped bounded preview plus the complete
content digest, and requires `c` then Enter. It sends only the retained bytes
and participation. A transport-unknown result fences another submission until
refresh; stdin is deliberately unavailable because the TUI retains the terminal.
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
`permission-interactive` reuses the same verifier as `permission-view`: two
stable participation/challenge reads surround a complete cursor-contiguous
agent-event read. It renders arbitrary capability, description, and option
bytes with explicit escapes, allows selection only from the retained option
set, and requires `c` then Enter. An unknown mutation outcome removes the
challenge and fences another confirmation until refresh.
`cancel-view` and `cancel-confirm` apply the same current-revision discipline to
an active turn. Moira binds the challenge to the exact attempt, method,
grounding, persona, session revision and turn, derives the cancel key, and
requires authenticated mutation access. The terminal never supplies a turn or
idempotency identity.
`cancel-interactive` is the reusable closed TUI form of that contract. It
double-reads the participation and server challenge, requires a distinct `c`
arm step followed by Enter, and submits only the retained confirmation digest.
Any mutation transport failure fences further confirmation until a read-only
refresh establishes current state; it never retries cancellation automatically.
`reattach-view` and `reattach-confirm` use the same stable server-challenge
discipline for an idle detached session. Exact confirmation replay returns the
already prepared operation, while transport uncertainty fences the interactive
control until refresh. The terminal can request recovery but cannot choose the
attempt, execution identity, provider process, remote session locator, context,
tools, or authority evidence.
Each interactive control reuses one participation connection while switching
between capability-shaped read and mutation clients. This matches the bounded
daemon's one-connection request loop and prevents a retained read connection
from starving its own mutation. If that connection becomes uncertain, the
control remains fenced and its next explicit refresh reconnects for read-only
recovery rather than resubmitting the mutation.

The end-to-end qualification tests start the real bounded Moira participation
and agent Unix profiles. They prove bounded prompt submission with durable
content and server-derived operation identities, view-to-confirm permission
selection with one Aegis authorization call, and view-to-confirm cancellation
with no invented turn/key or permission-authority call; all finish as exact
durable pending operations. A separate real-daemon gate qualifies the reusable
interactive participation projection with no authority call.
Real-daemon component gates additionally prove prompt and cancellation over one
participation connection and permission over one connection per participation
and agent daemon, including the exact Aegis-backed permission confirmation.
A fault-injected real Unix gate commits the prompt and its content first, drops
the reply, requires the CLI to report `outcome_unknown` as non-retry-safe, and
then proves that the same exact submission is recognized as a replay rather
than a second logical operation.
A companion reusable-component gate performs its initial stable double-read,
commits one prompt, loses that reply, visibly fences arm/confirm, and reconnects
only when the operator explicitly refreshes. The fresh stable read observes the
durable pending operation; the server accepts no second prompt frame.
Matching cancellation and permission gates retain one exact server challenge,
commit one derived operation, discard its reply, fence another confirmation,
and recover only by explicit refresh. Permission additionally proves exactly
one command-scoped authority decision and retains its decision, subject,
capability digest, and epoch; neither control receives a fresh challenge while
that operation is pending.
The read-only agent component separately exhausts a three-request daemon
connection, survives listener teardown/rebind over the same gateway state, and
reproduces its exact stable rendering only after explicit refresh, without a
mutation or inferred state transition.

Opening a selected saga reads bounded evidence and the bounded operator-receipt
page through `SagaReadOnlyClient`. Receipt disposition and authority decision
metadata are evidence only; the component neither constructs nor submits a
reconciliation operation.

The source dependency points at Moira's committed public client/protocol
boundary. Workbench records the exact Moira and Moira TUI revisions separately;
the reusable presentation still imports no coordinator, storage, daemon, peer,
or authority implementation.

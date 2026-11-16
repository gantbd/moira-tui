# Moira terminal roadmap

`moira-tui` is a reusable workflow presentation peer. Moira's standalone CLI
and Workbench are separate composition roots.

## Current slice

- Reusable Ratatui component over the public saga service port.
- Bounded saga inspection and reconciliation-state detail.
- Closed input and lifecycle outputs suitable for another host.
- Separate standalone CLI with explicit `interactive`, `headless`, and
  `capabilities` commands.
- Versioned `sagas.inspect.v1` headless envelope with explicit connection,
  transmission-uncertainty, refusal, request-limit, and invalid-reply states.
- No mutation or operator authority in the presentation package.
- Read-only `agent-inspect` and cursor-based `agent-events` headless commands
  over Moira's distinct bounded agent wire; no direct Daimon/provider access.
- Read-only `participation-inspect` over Moira's separate durable-participation
  wire, including Rune method, Dendrite grounding, persona, phase, retained
  evidence, and pending-operation kind.
- Read-only `permission-view` correlates a stable participation snapshot with a
  complete cursor-contiguous agent-event view and emits exact byte-preserving
  choices plus Moira's server-prepared, domain-separated confirmation
  fingerprint.
- [Qualified] Explicit `permission-confirm` submits only that fingerprint and one retained
  option. Moira revalidates current durable state, derives the opaque operation
  identities, and obtains Aegis authority; the terminal cannot inject either.
- [Qualified] `cancel-view`/`cancel-confirm` bind an active turn to current
  durable context and authenticated mutation access; Moira derives the cancel
  key and the terminal preserves transport uncertainty.

## Delivery order

1. [Qualified] Qualify the standalone CLI against Moira's real private saga
   daemon and SQLite saga owner.
2. [Qualified] Add the reusable component to Workbench interactive composition
   and route the same public client into Workbench headless aggregation. The
   real-daemon attach gate covers aggregate headless observation; a synthetic
   public-port gate covers host rendering without importing Moira internals.
3. Census and selected-saga evidence/operator-receipt views are qualified
   against the real daemon through the reusable component in standalone and
   Workbench paths. The interactive detail remains bounded and read-only.
4. Add reconciliation actions only after Moira supplies an exact confirmation,
   current-revision, command-scoped authorization, durable receipt, and
   outcome-unknown recovery contract suitable for an operator UI.
5. [Complete] Pin the clean Moira source receipt after its public service/client
   and adapter separation landed.

## Agent-provider participation

The terminal peer does not launch or speak directly to Codex, Claude, Gemini,
or another provider. Moira owns their workflow participation and reaches them
through Daimon's planned provider-neutral agent-session/ACP profile. This peer
will add provider-session, typed update, cancellation, and exact permission
confirmation views only through Moira's public client contract. Session,
typed-event, governed-participation, and exact permission-confirmation
observation now exist. Moira also retains exact Aegis-backed permission
selections, a terminal report-admission gate, and crash-conservative dispatch
fences. Permission and cancellation mutations use server-prepared
current-revision contracts and preserve transport uncertainty. Governed prompt
submission and the interactive participation UI remain open.

See `../moira/docs/agent-provider-interaction.md` for the cross-component
ownership and delivery plan.

Workbench may host or attach Moira, but workflow state, revisions, effect
identities, fences, authority, and reconciliation receipts remain Moira-owned.

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
- [Qualified] All private Unix client families retain a global request bound,
  invalidate a connection after transport or codec uncertainty, and reconnect
  only for the next explicit command. A real-socket participation gate proves
  a lost prompt reply is not retried and a later read recovers on a fresh
  connection.
- Saga mutation is limited to the explicit server-prepared
  `reconciliation-confirm` CLI gate and its reusable arm/confirm peer.
  Operator authority remains daemon-owned;
  the presentation package carries only the exact revision, digest,
  disposition, and derived receipt identity returned by Moira.
- [Qualified] The reusable saga list marks a sparse bounded set of up to 16
  sagas. Batch reconciliation preserves the owner's per-saga contract: every
  child is re-inspected just-in-time because a preceding reconciliation changes
  the global storage revision, each successful child reads its own durable
  operator receipt, and an unknown child stops all later sends until exact
  receipt recovery. Partial, refused, not-sent, receipt-pending, and unknown
  children remain distinct; no aggregate atomic-success claim is made.
- Read-only `agent-inspect` and cursor-based `agent-events` headless commands
  over Moira's distinct bounded agent wire; no direct Daimon/provider access.
  A real-daemon gate preserves the complete eight-bit capability projection in
  the versioned headless JSON envelope.
- [Qualified] Reusable read-only `AgentComponent` and `agent-interactive` host
  over the same public wire. Stable double-read observation rejects cursor gaps
  and changing snapshots, marks page truncation, preserves byte boundaries in
  escaped previews, displays all provider-neutral session capabilities without
  inventing a control, traverses only server-derived cursor pages, and passes a
  one-connection real-daemon gate. A second gate exhausts that bounded
  connection, tears down and rebinds the real daemon around the same gateway,
  then reproduces the exact snapshot only on explicit refresh while refusing
  every mutating gateway command.
- [Qualified] The public agent view carries `ReattachRequired` without
  collapsing it into ready or outcome-unknown. A read-only worker inspection
  durably marks the exact detached participation; server-prepared
  `reattach-view`/`reattach-confirm` and reusable arm/confirm controls derive
  operation identities from the retained methodical context and expose no
  automatic resume, raw authority/attempt input, or provider locator. Exact
  confirmation replay and unknown-outcome fencing are covered. Moira's
  `qualify-agent-reattach-recovery` target composes this real-daemon CLI gate
  with the separately owned gates and an integrated Moira worker journey through
  a killed and reopened real Daimon executable.
- Read-only `participation-inspect` over Moira's separate durable-participation
  wire, including Rune method, Dendrite grounding, persona, phase, retained
  evidence, and pending-operation kind.
- [Qualified] Reusable read-only `ParticipationComponent` and standalone
  `participation-interactive` host over the same public port, with refresh and
  close as its only controls, the complete eight-fact capability projection,
  and a real-daemon rendering gate.
- Read-only `permission-view` correlates a stable participation snapshot with a
  complete cursor-contiguous agent-event view and emits exact byte-preserving
  choices plus Moira's server-prepared, domain-separated confirmation
  fingerprint.
- [Qualified] Explicit `permission-confirm` submits only that fingerprint and one retained
  option. Moira revalidates current durable state, derives the opaque operation
  identities, and obtains Aegis authority; the terminal cannot inject either.
- [Qualified] `permission-interactive` uses the same complete-event correlation verifier,
  preserves arbitrary labels with byte escapes, selects only a retained option,
  requires arm then confirm, and fences unknown outcomes. Its synthetic
  public-port and real-daemon gates are complete. A lost-reply gate proves one
  command-scoped authority decision, durable attribution, no second selection,
  and explicit-refresh recovery without a fresh pending-state challenge.
- [Qualified] `cancel-view`/`cancel-confirm` bind an active turn to current
  durable context and authenticated mutation access; Moira derives the cancel
  key and the terminal preserves transport uncertainty.
- [Qualified] `cancel-interactive` adds a separate reusable arm/confirm control over that
  exact server challenge. It double-reads before presentation and fences an
  unknown mutation outcome until refresh; synthetic public-port qualification
  and real-daemon gates are complete. Its lost-reply gate proves one durable
  derived cancel operation, no second confirmation, and explicit-refresh
  recovery without a fresh pending-state challenge.
- [Qualified] `prompt-submit` sends only a participation and 1--61440 exact
  bytes behind explicit confirmation. Moira durably commits the content, then
  derives the digest-only operation's turn, execution, key, and retained
  attempt from current methodical context.
- [Qualified] `prompt-interactive` adds a stable governed-state preview, escaped bounded
  byte preview, full content digest, arm/confirm submission and unknown-outcome
  fence. Its synthetic public-port and real-daemon gates are complete. A
  fault-injected real Unix gate additionally proves durable commit before a
  lost reply, explicit non-retry-safe classification, and exact replay without
  a second logical operation. A reusable-component fault gate additionally
  proves the unknown fence blocks re-arm and an explicit refresh reconnects to
  observe that durable pending operation without another prompt frame.
- [Qualified] `deliberation-inspect` and the reusable
  `DeliberationComponent` expose exact personas, grounding, claims, candidates,
  report evidence/dissent, and retained proof/authorized-choice/maybe/mu
  resolution through Moira's distinct canonical wire and read-only private
  daemon. Both double-read the registry revision and refuse a mixed snapshot;
  neither recomputes the Pareto frontier nor constructs authority.
- [Qualified] Capability discovery lists the exact versioned schemas for every
  standalone headless governed control, including prompt submission and the
  server-prepared reconciliation, permission, and cancellation view/confirm
  pairs. Discovery conveys no workflow or provider authority.

## Delivery order

1. [Qualified] Qualify the standalone CLI against Moira's real private saga
   daemon and SQLite saga owner.
2. [Qualified] Add the reusable component to Workbench interactive composition
   and route the same public client into Workbench headless aggregation. The
   real-daemon attach gate covers aggregate headless observation; a synthetic
   public-port gate covers host rendering without importing Moira internals.
3. [Qualified] Census and selected-saga evidence/operator-receipt views are qualified
   against the real daemon through the reusable component in standalone and
   Workbench paths. The interactive detail remains bounded and read-only.
4. [Qualified] Add reconciliation actions only after Moira supplies an exact confirmation,
   current-revision, command-scoped authorization, durable receipt, and
   outcome-unknown recovery contract suitable for an operator UI.
   The headless `reconciliation-view`/`reconciliation-confirm` path and reusable
   arm/confirm component now have this contract. Synthetic qualification proves
   unknown outcomes fence further mutation until exact durable-receipt recovery,
   a real-daemon gate covers stable challenge presentation, and a fault-injected
   real socket proves a durably applied reconciliation survives reply loss and
   is recovered only by its exact operator receipt after reconnect.
5. [Qualified] Add persona, evidence, dissent, Pareto-frontier, and resolution
   views only after Moira exposes its public deliberation contract through a
   bounded private daemon. Headless and reusable interactive paths now pass the
   real SQLite-reopen daemon gate.
6. [Complete] Pin the clean Moira source receipt after its public service/client
   and adapter separation landed.

## Agent-provider participation

The terminal peer does not launch or speak directly to Codex, Claude, Gemini,
or another provider. Moira owns their workflow participation and reaches them
through Daimon's implemented provider-neutral agent-session profile and its
selected provider adapters. This peer exposes provider-session, typed update,
cancellation, and exact permission-confirmation views only through Moira's
public client contract. Session,
typed-event, governed-participation, and exact permission-confirmation
observation now exist in both headless and reusable interactive forms. Moira
also retains exact Aegis-backed permission
selections, a terminal report-admission gate, and crash-conservative dispatch
fences. Permission and cancellation mutations use server-prepared
current-revision contracts and preserve transport uncertainty. Governed prompt
submission is qualified through the real bounded socket and content store. The
read-only interactive participation UI is qualified. Interactive permission
prompt, permission and cancellation controls now have closed arm/confirm and
outcome-unknown fencing under synthetic public-port and real-daemon gates. Each
control reuses one participation connection so the bounded daemon cannot be
starved by a second retained connection.

See `../moira/docs/agent-provider-interaction.md` for the cross-component
ownership and delivery plan.

Workbench may host or attach Moira, but workflow state, revisions, effect
identities, fences, authority, and reconciliation receipts remain Moira-owned.

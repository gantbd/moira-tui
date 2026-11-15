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

## Delivery order

1. [Qualified] Qualify the standalone CLI against Moira's real private saga
   daemon and SQLite saga owner.
2. [Qualified] Add the reusable component to Workbench interactive composition
   and route the same public client into Workbench headless aggregation. The
   real-daemon attach gate covers aggregate headless observation; a synthetic
   public-port gate covers host rendering without importing Moira internals.
3. Census is now qualified in the reusable component and standalone headless
   path. Selected-saga evidence and operator-receipt views remain read-only
   projection work.
4. Add reconciliation actions only after Moira supplies an exact confirmation,
   current-revision, command-scoped authorization, durable receipt, and
   outcome-unknown recovery contract suitable for an operator UI.
5. Pin a clean Moira source receipt once its active service/client work lands.

## Agent-provider participation

The terminal peer will not launch or speak directly to Codex, Claude, Gemini,
or another provider. Moira owns their workflow participation and reaches them
through Daimon's planned provider-neutral agent-session/ACP profile. This peer
will add provider-session, typed update, cancellation, and exact permission
confirmation views only after those commands and projections exist in Moira's
public client contract.

See `../moira/docs/agent-provider-interaction.md` for the cross-component
ownership and delivery plan.

Workbench may host or attach Moira, but workflow state, revisions, effect
identities, fences, authority, and reconciliation receipts remain Moira-owned.

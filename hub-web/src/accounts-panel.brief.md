# Brief: Accounts across machines

## Single idea

An operator can inspect each machine's harness accounts and clean up a named profile without leaving Commander or guessing which login New session will use.

## Hero form

A machine-grouped account ledger in the existing body-mounted dialog: harness headings, identity and state first, usage and reset facts second, and explicit row actions last. One machine's failure leaves other machines readable.

## Emotional register

Quiet and exact. Reuse Commander Glass, semantic ink, rule, spacing, critical and focus tokens from hub-web/DESIGN.md; deletion is an explicit decision, never an icon-only shortcut.

## Distinctive move

Each account explains both its selection state and its evidence: Default and Recommended remain distinct; cached capacity carries its observation time and local 12-hour reset times. A successful mutation refreshes that machine and New session's account choices without losing the selected project.

## Deliberately omitted

No credential fields or absolute profile paths cross the wire. No subscription usage endpoint, guessed usage, browser-driven login, new palette, or provider behavior inferred from another harness. Unknown capacity says unknown. Protected or unsupported operations explain why they are unavailable.

## State and component ownership

A body-mounted AccountsSheet owns open state, per-machine loading/errors, current row edit/confirmation, request cancellation and mutation feedback. main.ts owns paired machines and connections. Connection owns authenticated HTTP. A machine-local account interface owns safe discovery/projection, allowlisted mutation dispatch, lifecycle guards, policy/default persistence and requested/outcome auditing. New session gains an explicit accountsChanged(machineId) invalidation method; ordinary shell refresh currently ignores account changes when machine/scopes/connectivity stay stable.

## Interface direction

Use GET /v1/accounts and POST /v1/accounts/operations with typed operations and provider-qualified aliases, never caller-supplied filesystem paths. Preserve /v1/launch/profiles as a compatibility projection. Snapshot rows carry structured usage windows, observation/reset instants, independent state flags, recommendation/default and supported-operation reasons. Remove requires the typed account identifier; Disable requires confirmation; the server validates both. Mutations return a receipt or an honest partial-failure refusal; the client then reloads that machine and invalidates New session, and authorization/audit admission precedes changes.

Three independent read-only proposals explored a two-entry-point interface, an extensible capability-bearing interface and the common UI caller. The coherent choice is the small two-endpoint interface with explicit per-provider capabilities and one client invalidation hook. Separate provider-specific public endpoints lose shared authorization/audit locality; a global browser account store adds synchronization ownership that two existing sheets do not need. The local implementation must earn its seam by hiding discovery, guards and policy writes rather than forwarding arbitrary commands.

## Resolved provider and authorization constraints

Supervisor decision (2026-10-28): listing requires machine:read and mutations require factory:manage. Reuse the existing registry without adding a credential reader; its existing Codex metadata read is tracked separately. Grok appears as a single read-only row, with unknown login and capacity and explicit unsupported-operation reasons. It has no named lifecycle or fabricated login command. Claude/Codex mutations reuse the CLI lifecycle, including active-account, main-account and shared-base protections. Requested and outcome audit surround mutations; an uncertain outcome forces a refresh and never claims rollback.

## Acceptance and proof

Five operation cases exercise the real route allowlist, authorization and disposable profile effects, plus denied and confirmation paths. UI tests exercise states and New session invalidation. Run capped named Rust tests, affected vitest modules, app/e2e tsc and producer-selected journeys. A supervisor-built exact-tip real hub candidate must prove real accounts match the CLI, every mutation is audited and reflected in New session without reload. Capture 1280 and 390 in light/dark, keyboard focus, confirmations, reduced motion, forced colors and strict visual QA. No synthetic backend result is a substitute for the real-candidate criterion.

## Critique

Pending implementation and rendered evidence.

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

### Header entry repair

The header must reveal Accounts before the operator opens the panel. Reuse the existing user-group SVG inside the shared `icon-button`, retaining its Accounts tooltip and accessible name. Keep the action group intact and allow it below the full wordmark when the rail is too narrow. Existing `--space-2` gaps and `--space-4` desktop inset give both icons breathing room; phone safe-area rules retain their existing inset. This is a header repair only; account layout and shared tokens stay with their separate design work.

The visual argument is the visible group icon beside settings, fully inside the glass panel. Its register remains quiet and exact; the distinctive choice is to preserve the full brand and the complete action group as each row's unit. No new palette, token, account behavior or navigation surface is introduced. Design source: `hub-web/DESIGN.md@3c25c3fb3`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Machine/provider ledger joins state and dated capacity; Default and Recommended stay distinct. |
| Fit | 5 | Commander Glass and the existing ink, action, critical and focus tokens. |
| Hierarchy | 4 | Identity/state, then capacity, then explicit account actions. |
| Craft | 4 | Eight strict panel/confirmation captures pass at 1280/390 in light/dark with zero findings; the last phone row is reachable by wheel. |
| Accessibility | 4 | Keyboard confirmation and focus return, named controls, read-only restrictions, and forced colors/reduced motion/more contrast captures. |

Evidence: task cas-e5df `LEDGER.md` and `qa/bundle.json`, with exact candidate and native embedded-asset build recorded in the ledger. Eight real hub cells prove CLI parity, every mutation and audit, read-only denial and New session refresh. The 201 producer-selected browser cases, 189 Rust lib cases, 37 hub lifecycle integration cases and 75 UI cases pass. Native login/usage fixture boundaries are disclosed in the ledger; no live subscription reading is claimed.

Strict QA caught the generic dialog overflow rule overriding the panel on a phone. The fix gives the panel an explicit open-state scroll rule; the original failure and a wheel regression that failed before the fix are retained. Two capture attempts reached the existing pairing throttle; resetting the disposable fixture between independent cases corrected the harness, and those failed captures alone were repeated. No visual findings were suppressed.

The panel and stylesheet load on demand. Vite reports initial app.js at 199.88 kB gzip, with 0.88 kB lazy Accounts CSS. The existing main stylesheet is 38.03 kB gzip, above the generic 30 kB budget; this task preserves that baseline. Field LCP/INP/CLS remain the release performance lane's measurements. No image assets or decorative motion were added. The existing JavaScript declaration and print behavior are checked by strict QA.

Supervisor-approved demo clarification preserves the CLI's protected main account: delete a stale named duplicate after confirming, then observe its absence in New session. No lifecycle guard was weakened.


The lazy Accounts stylesheet uses the hub's hashed chunk convention. The dist manifest regression caught Vite otherwise emitting an unservable app2.css; the final QA controller proxies actual candidate-embedded Commander assets rather than serving a separate dist directory. The original CI failure and local red manifest test are retained.

### Confirmation editor correction

Verifier ver-0e147d7370b5 invalidated the prior completion claim: the shared dialog form grid overrode the editor's hidden attribute. The editor now has a scoped display-none rule; Cancel, dialog close/reopen and mutation settlement clear its heading, explanation, label, value and validation state. The existing browser journey now checks rendered visibility after Cancel/remove and for read-only pairings, rather than trusting the attribute. The original failure remains evidence. Native ACC-02/ACC-06 recaptures and the correction's final visual/craft verdict are deferred to batch QA round 2; the earlier critique scores do not prove this correction.

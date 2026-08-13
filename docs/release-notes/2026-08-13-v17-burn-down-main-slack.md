# Slack draft — v17 burn-down main landing

Channel: `#cas-internal` (`C0B44GUKDK2`). Deploy label: **Live on production**.

**Status:** Draft. The four message bodies below are postable after validation.

=== MESSAGE 1 (user top-level) ===
Live on production — **User** — Before, everyday searches and completed work could look broken or disappear from view; now, searches understand common punctuation and delivery status stays visible and accurate.

=== MESSAGE 2 (user reply to MESSAGE 1) ===
- Was: a search containing a time, web address, or code-style name could fail because of its colon. Now: unfamiliar colon-bearing text is searched literally, while recognized filters still work.
- Was: delivery notes and receipts stored with a task could be found only by browsing files. Now: supported text artifacts are indexed and searchable with their task attached.
- Was: a custom-profile session could begin without the project context it needed. Now: that context is included at launch even when the normal startup event is skipped.
- Was: newly started work could be based on an older project snapshot and miss commits it depended on. Now: a clean base is refreshed from its recorded parent before the new checkout is created, while divergent history remains protected.
- Was: a permission intended for a real fallback to the main branch could also block an explicitly configured non-main destination. Now: that permission is required only for a genuine main-branch fallback, and the fallback is called out clearly.
- Was: task details did not say where delivery would land, including when no destination was configured. Now: task details show the repository and branch, or explicitly warn that the main-branch fallback applies.
- Was: completed work merged as a single combined commit could still be reported as stranded. Now: the content is reconciled before a delivery is blocked, while genuinely missing changes still stop the close.
- Was: automated checks could borrow settings from the live session and report a result that did not stand on its own. Now: those checks begin clean and prove the intended behavior independently.

=== MESSAGE 3 (dev top-level; new top-level, not a reply) ===
Live on production — **Dev** — Before, search parsing, artifact indexing, base selection, and delivery reconciliation had separate edge-case gaps; now, PR #319 makes those paths explicit, content-aware, and fail-safe.

=== MESSAGE 4 (dev reply to MESSAGE 3) ===
- Was: Tantivy parsed every colon-bearing token as a field expression and rejected unknown fields. Now: schema-known fields remain strict, while timestamps, URLs, and Rust-style paths are quoted as literal terms across the core, shared search crate, and MCP boundary.
- Was: durable task artifacts were outside the search corpus. Now: bounded Markdown, text, and JSON discovery feeds the shared index at close and during reindex, with task identity and rendered paths preserved.
- Was: custom-config Claude launches relied entirely on SessionStart dispatch for ambient context. Now: the guaranteed launch prompt carries the existing context bundle when a custom config directory is used and the startup event is absent.
- Was: checkout creation could use a stale local delivery base. Now: clean fast-forwardable base refs advance atomically from the recorded parent before provisioning; divergent refs remain untouched with a loud stale-base warning.
- Was: `allow_trunk` gated declared non-trunk WorkTargets as well as true trunk fallback. Now: declared destinations merge without that flag, while actual fallback requires explicit authorization and renders the resolved target loudly.
- Was: `task show` omitted its WorkTarget and made an unset destination ambiguous. Now: it renders `repository @ branch` or an explicit `(none — trunk fallback)` line through the shared handler.
- Was: delivery status treated rewritten squash history as missing solely because commit ancestry was lost. Now: each child anchor is content-reconciled against local and remote parents before the close gate declares it stranded.
- Was: authority-boundary integration tests inherited ambient `CAS_*` variables. Now: the child test environment scrubs those variables before exercising the public boundary, making the result hermetic.

## Posting sequence

1. Read `#cas-internal` and confirm these exact bodies have not already been posted.
2. Post MESSAGE 1 as a new top-level message and capture its timestamp.
3. Post MESSAGE 2 as the one reply to MESSAGE 1.
4. Post MESSAGE 3 as a new top-level message and capture its timestamp.
5. Post MESSAGE 4 as the one reply to MESSAGE 3.
6. Append `## POSTED` with UTC timestamps and all four permalinks, then verify both threads.

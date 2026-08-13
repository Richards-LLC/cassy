# Slack draft — Code health v1 main landing

Channel: `#cas-internal` (`C0B44GUKDK2`). Deploy label: **Live on production**.

**Status:** POSTED. Both top-level messages and their single replies were verified against the live channel.

=== MESSAGE 1 (user top-level) ===
Live on production — **User** — Before, task context could miss saved guidance and some storage failures could end abruptly; now, the right guidance is recalled more reliably and store errors fail cleanly.

=== MESSAGE 2 (user reply to MESSAGE 1) ===
- Was: task-time context could fill up with generic history while leaving out saved preferences or memories that directly matched the work. Now: saved operator guidance and task-specific memories stay visible, even when the underlying search path changes.
- Was: poisoned internal store locks could trigger abrupt crashes. Now: those failures are returned as handled errors instead of panics.

=== MESSAGE 3 (dev top-level; new top-level, not a reply) ===
Live on production — **Dev** — Before, core maintenance and task-context behavior relied on scattered, weakly typed contracts; now, PR #295 centralizes enforcement and makes focused recall consistent across scoring channels.

=== MESSAGE 4 (dev reply to MESSAGE 3) ===
- Was: workspace crates repeated dependency versions and had no shared lint foundation. Now: `anyhow` and `thiserror` versions are unified in workspace dependencies, and every member inherits the workspace lint configuration.
- Was: production store code contained hundreds of panic-prone lock unwraps. Now: shared lock handling converts poisoning into `StoreError`, and `cas-store` denies production `clippy::unwrap_used` regressions.
- Was: Claude, Codex, and Grok behavior lived in backend-specific methods on shared enums. Now: `cas-mux` exposes one `Backend` contract with one implementation per CLI, so another backend can be added through an implementation and registration instead of expanding shared enum methods.
- Was: eleven task-lifecycle gates returned unstructured strings. Now: they return a typed lifecycle error enum while preserving stable messages at the MCP boundary.
- Was: task-focused ambient context did not carry the task identifier and budget through the public search route, so preference directives and task-content matches could be displaced. Now: the route supplies focused task content and the requested budget to context selection.
- Was: lexical overlap was divided by the entire generated query, so duplicated task text and the working directory could erase a real match on the fallback scoring path. Now: bounded absolute overlap evidence keeps multi-term task matches ahead of generic high-importance memories across BM25 and fallback scoring.

## Posting sequence

1. Read the recent `#cas-internal` messages and confirm these bodies have not already been posted.
2. Post MESSAGE 1 as a new top-level message and capture its timestamp.
3. Post MESSAGE 2 as the one reply to MESSAGE 1.
4. Post MESSAGE 3 as a new top-level message and capture its timestamp.
5. Post MESSAGE 4 as the one reply to MESSAGE 3.
6. Append `## POSTED` with UTC timestamps and all four permalinks, then verify both threads.

## POSTED

Channel: `#cas-internal` (`C0B44GUKDK2`).

| Message | UTC timestamp | Permalink |
| --- | --- | --- |
| User top-level | 2026-08-13T23:32:06.707049000Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786663926707049 |
| User reply (Was → Now) | 2026-08-13T23:32:10.849759000Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786663930849759?thread_ts=1786663926.707049&cid=C0B44GUKDK2 |
| Dev top-level | 2026-08-13T23:32:14.223359000Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786663934223359 |
| Dev reply (Was → Now) | 2026-08-13T23:32:23.880599000Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786663943880599?thread_ts=1786663934.223359&cid=C0B44GUKDK2 |

# Team Ticket Explorer — CLI client behaviors

The team ticket explorer lets you and your teammates share task visibility through
the web UI (petra-stella-cloud) while continuing to work in the Cassy CLI. The cloud
half (read API, comments, media upload, web close, canonical resolver) ships
server-side; this page documents what the **CLI** does to stay in sync with it.

EPIC cas-71f7. Server contract: `docs/requests/SHIPPED-team-ticket-explorer.md`.

## 1. Canonical project id is adopted automatically (cas-8ca5)

When you `cas cloud push` (team scope), the CLI now sends your normalized git
remote (e.g. `github.com/richards-llc/ozer-health`). The server maps it to the
team's canonical project and echoes back `{ canonical_id, git_remote }`.

If the returned `git_remote` matches your local `origin` (case-insensitive), the
CLI **adopts** the returned `canonical_id` into `.cas/config.toml`
(`[project] canonical_id`). This stops an unpinned machine from silently syncing
a fragmented per-remote bucket instead of the team's shared project.

- Adoption only happens when the remotes match — a shared machine with a
  different `origin` is never silently re-homed onto someone else's project.
- A `[Cassy sync] adopted team canonical project id '<id>' (matched git remote)`
  line is printed when it happens; nothing prints when already pinned.

## 2. Comments from the web show up in `task show` (cas-7d54)

Comments authored in the web ticket explorer are surfaced read-only when you run
`task show` (or `mcp__cas__task action=show`). Each comment shows the author
email, timestamp, body, and any attachments (image / video / link) as
`[kind] url` links.

- **Read-only:** comments are authored in the web UI. There is no CLI write path
  in v1.
- **Best-effort:** if you are not logged in, have no team resolved, or are
  offline, the comments section is simply omitted — it never blocks or fails
  `task show` (4-second timeout, fetched off the async runtime).

## 3. A teammate's web close reconciles on pull (cas-fc52)

When a teammate closes a task in the web UI, the server writes a soft tombstone
(`closed_via = "web"`) that arrives in your next `cas cloud` pull. The CLI honors
it as a real local close:

- The task is set to **Closed** with the teammate's close reason, even if your
  local copy looks newer (a web close is an explicit instruction).
- The task's `assignee` is cleared so no stale ownership lingers. Any separate
  agent-lease row is reclaimed by the normal lease GC — a closed task is not
  claimable.
- Only `closed_via == "web"` triggers this, so the CLI never reconciles its own
  pushed closes (no loop, no double-close). Re-pulling an already-closed task is
  a no-op.

## Not in v1

- **No CLI-authored comments** (read-only mirror). If we want offline
  comment authoring later, the cloud team adds an id-accepting upsert + an
  incremental `comments?since=` feed (declined for v1).
- **No bulk comment pull** — comments are fetched per task on demand.

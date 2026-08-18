---
date: 2026-05-06
topic: cas-web-ticket-explorer
focus: web-based ticket/task explorer for Petra Stella team members, hosted on petra-stella-cloud
---

# Ideation: Cassy Web Ticket Explorer (Team Surface)

The user's complaint that started this — "I can't see what's in tickets and I can't interact with them in the TUI" — is real, but the answer is *not* a localhost browser tab for a single operator. The answer is a **team-facing surface on petra-stella-cloud** where any Petra Stella team member can find, view, comment on, and close any task across any of the team's projects.

## Decisions Locked

| # | Decision | Reasoning |
|---|---|---|
| 1 | **Cloud-served, not local-bridge** | Audience is the Petra Stella team (multiple humans, multiple machines). A localhost bridge serves one user on one machine. |
| 2 | **Hosted inside the existing petra-stella-cloud Next.js app on Vercel** | Auth, team membership, project registry, and Vercel deploy pipeline are already shipped. One app, one deploy, one login. |
| 3 | **All team projects in scope for v1** | Cloud already auto-registers projects on first push (`projects` table, migration 0007). The data is already there — it just needs to be used. |
| 4 | **Read + light-write** | Team members can: close tasks (with reason), comment, attach screenshots. Field-level edits (priority, title, AC) deferred. |
| 5 | **Comments are a first-class entity** | New `task_comments` table on both sides. Overloading `Task.notes` would lose authorship and round-trip badly. |
| 6 | **Screenshots via Vercel Blob with signed-URL uploads** | Already a Vercel app. `@vercel/blob/client` `upload` helper handles the signed-URL dance. Bytes never touch the API server. |
| 7 | **Cloud writes are soft signals; the local CLI reconciles** | Cloud has no awareness of agent leases or worktrees. A cloud-side close becomes a tombstone that propagates via existing `cas cloud sync` (LWW), and the local CLI handles lease release on its end. |
| 8 | **`MDXEditor` for compose, `react-markdown` + `remark-gfm` for render** | Purpose-built for markdown (vs TipTap's rich-text-first), built-in `imageUploadHandler` for Vercel Blob, lean bundle. |

## Grounding Summary

**Project shape.** Cassy is a Rust monorepo at `cas-src/` with `cas-cli` and 16+ crates under `crates/`. SQLite at `.cas/cas.db` is the local source of truth. Petra Stella Cloud is a Next.js app deployed on Vercel that already serves as the team-collaboration backend.

**Task model** (`crates/cas-types/src/task.rs`): `id`, `title`, `description`, `design`, `acceptance_criteria`, `notes`, `status` (Open / InProgress / Blocked / Closed / PendingSupervisorReview), `priority` (P0–P4), `task_type` (Task / Bug / Feature / Epic / Chore / Spike), `assignee`, `labels`, timestamps, `branch`, `worktree_id`, `deliverables` (`files_changed`, `commit_hash`, `merge_commit`, `review_envelope`), `external_ref`, `demo_statement`, `execution_note`, `epic_verification_owner`, `pending_verification`, `pending_worktree_merge`, `team_id`. Dependency graph in a separate table.

**What the cloud already does** (`docs/FEATURE-REQUEST-TEAM-PROJECT-MEMORIES.md`):
- `sync_entities` table holds team+project-scoped data (`team_id`, `project_id`, JSONB `data`).
- `projects` table auto-registers when any team member pushes with `project_canonical_id`.
- `GET /api/teams/{tid}/projects` lists team projects with contributor + memory counts.
- `GET /api/teams/{tid}/projects/{pid}/memories` returns project-scoped memories with privacy filters.
- All endpoints check team membership server-side (403 if not a member).

**The pre-requisite that doesn't exist yet:** tasks don't sync to cloud. Memories do. Adding `entity_type='task'` to the existing sync pipeline is the gating work.

**What's getting deleted** (`docs/spikes/2026-05-01-factory-agent-teams-enrollment-spike.md`): the `~/.claude/teams/<session>/` factory-agent-teams enrollment is being dropped (~1700 LOC removal). "Team" in this doc means **cloud team** (org-like entity in petra-stella-cloud), never **factory team** (transient multi-agent runtime — gone).

**Known friction this addresses:**
- TUI hides full task content; only way to read tickets is `cas task show <id>` from a shell.
- No way for one team member to see what another is working on.
- Closed tasks become forensically inert — the rich `deliverables` block is invisible.
- No place for a human to leave a comment that survives the local SQLite.
- Screenshots get pasted into Slack and lost from the task record forever.

**Leverage points:** The cloud has already solved auth, team membership, project registration, and the sync pipeline for memories. Building tasks on the same substrate is mechanical, not novel.

## v1 Work Breakdown

This isn't a list of alternatives anymore — it's the cohesive feature, broken by code surface. Each surface is a candidate for one or more EPICs.

### A. Server (`petra-stella-cloud`)

1. **Extend `sync_entities` to accept `entity_type='task'`** — likely trivial since storage is JSONB. Add filtered indexes if query performance demands.
2. **`GET /api/teams/{tid}/projects/{pid}/tasks`** — list with `?status=&since=&q=&assignee=&limit=&cursor=`. Mirror the memory endpoint's auth and privacy filters.
3. **`GET .../tasks/{id}`** — single task with full field set + comments inline.
4. **`POST .../tasks/{id}/close`** — body: `{ close_reason }`. Updates the task row; cloud-as-soft-signal (see decision 7).
5. **New `task_comments` table** + `GET/POST .../tasks/{id}/comments`. Schema: `id`, `task_id`, `team_id`, `project_id`, `author_user_id`, `body` (markdown), `attachments` (JSONB array of `{url, alt, width, height}`), `created_at`, `updated_at`.
6. **`POST /api/uploads/signed-url`** — issues a Vercel Blob signed URL for client-direct upload. Returns `{ uploadUrl, publicUrl }`.
7. **Privacy decision** — extend the existing memory privacy filter to tasks: filter `execution_note` and any field flagged `agent-internal` from team responses unless the requester is the assignee. Mirror the `?exclude_user_type` opt-out.

### B. Cassy CLI (`cas-src`)

1. **Push tasks on `cas cloud sync`** — extend the team push pipeline (`cas-cli/src/cloud/syncer/team_push.rs`) to include `entity_type='task'` rows alongside memories.
2. **Pull task changes** — extend `pull.rs` to handle task entity type with LWW merge against local SQLite.
3. **Reconcile cloud closes locally** — when sync pulls a `Closed` status for a task that locally has a held lease, surface a reconciliation prompt or auto-release the lease. New code in `cas-cli/src/cloud/syncer/`.
4. **Local `task_comments` table in `crates/cas-store/`** — schema parallels server. Sync alongside tasks.
5. **New entity type in `crates/cas-types/src/entry.rs`** for task and comment serialization shapes.

### C. Web UI (in `petra-stella-cloud` Next.js app)

1. **Routes:** `/teams/{slug}/tasks` (cross-project), `/teams/{slug}/projects/{slug}/tasks` (per-project), `/teams/{slug}/projects/{slug}/tasks/{id}` (detail with permalink).
2. **Project switcher** — dropdown reading from existing `/api/teams/{tid}/projects`.
3. **Status-grouped board** — three columns: *Open · In Progress · Done*. Counts visible. Sticky filters in URL.
4. **Filters** — chip filters for `priority`, `task_type`, `label`, `assignee`, plus a "mine" toggle and a search box.
5. **Search** — full-text against `title`, `description`, `design`, `acceptance_criteria`, `notes`. Postgres FTS server-side is the cheap path; revisit if relevance is poor.
6. **Detail view** — full markdown render of `description / design / acceptance_criteria / notes / demo_statement / deliverables` via `react-markdown` + `remark-gfm`. Code blocks syntax-highlighted. Deliverables block shows `files_changed` tree, links to `commit_hash` and `merge_commit` in GitHub.
7. **Comment composer** — `MDXEditor` instance with image-upload handler wired to the signed-URL endpoint. Drag-and-drop and paste both upload via `@vercel/blob/client`.
8. **Comment thread** — render markdown with embedded blob images, author avatar, timestamp, permalink per comment.
9. **Close button** — confirms with a `close_reason` dropdown ("Done", "Won't fix", "Duplicate", "Obsolete") + free-text note.
10. **Permalinks** — `/teams/{team-slug}/projects/{project-slug}/tasks/{task-id}` resolves cleanly for Slack/email pasting.

### Cross-cutting

- **Conflict semantics for status** — agreed in decision 7. Document explicitly in the brainstorm phase.
- **Comment edit / delete** — out of scope for v1. Append-only.
- **Real-time updates** — out of scope. 30s polling is fine; revisit if it becomes painful.
- **Notifications** (Slack, email) — out of scope for v1.

## Deferred Ideas (Held for Later Phases)

These survived the original adversarial filter but don't fit the team-collaboration framing. Held for a follow-up "operator console" effort or v2+ of this surface:

| Idea | Why deferred |
|------|--------------|
| Operator-console / lease + stalled + worktree anomaly view | Single-operator real-time tool. Cloud doesn't sync lease state. Belongs in a separate local-bridge effort. |
| Pending-supervisor-review inbox as front door | Operator-shaped. Could become a v2 cloud feature once review_envelope sync exists. |
| Trace-first task view (couple `cas-recording` × tasks) | Recordings live locally and are large. Cloud-streaming recordings is a real undertaking; defer to v3+. |
| Dependency graph with cycle / orphan detection | Real value, but not in the "find and view tickets" envelope. v2 candidate. |
| Dual-surface renderer (`cas-view` crate, TUI/web parity) | Irrelevant once the web UI is a separate Next.js app, not a TUI variant. |
| Local-bridge `/v1/tasks` API + SSE event stream | Belongs to the deferred operator-console effort, not this team-collaboration EPIC. |

## Rejection Summary

| # | Idea | Reason Rejected |
|---|------|-----------------|
| 1 | Reviewless trust streaks | Premature — no trust-signature data collected yet |
| 2 | Outcome-to-graph compiler | Drifts off topic; authoring tool, not explorer |
| 3 | Acceptance-criteria as live probes | High value but huge scope (probe DSL + runtime); separate brainstorm |
| 4 | Worth-doing score / suggestion-as-entity | Premature — no ranking signal collected |
| 5 | Boring-task auto-pilot | Out of scope; factory-policy feature |
| 6 | Git-op dependency auto-resolution | Too expensive (git hooks + edge inference); separate concern |
| 7 | Branch-centric map | Speculative viz; no clear pain it uniquely solves |
| 8 | Anti-linear spatial canvas | Fun, expensive, no demonstrated payoff |
| 9 | Memorable phonetic nicknames | Brainstorm variant inside MVP, not its own line item |
| 10 | Agents-as-audience console | Duplicates existing MCP `task` tool surface |
| 11 | Epic-as-document view | Expensive editor work; brainstorm variant of MVP |
| 12 | Task embedding projection / cluster map | Premature; needs embedding infra; speculative value |
| 13 | Review-envelope mining dashboard | Solves a problem of scale that doesn't exist yet |
| 14 | Plain-text markdown round-trip | Conflicts with SQLite-as-truth model; expensive |
| 15 | Per-team cross-team aggregation | User is single-team for now |
| 16 | Multi-team file-conflict detection | Speculative for current usage |
| 17 | External-ref / GitHub PR reconciliation | Ancillary; could fold into deferred review-inbox effort |
| 18 | Demolition mode (close-by-staleness) | Folded into deferred anomaly console |
| 19 | Spec / skill / pattern cross-reference panel | Brainstorm variant of detail view |
| 20 | Agent-note reader mode | Subsumed by detail view's full markdown render |
| 21 | Deliverables forensics view | Subsumed by detail view's deliverables block |
| 22 | Inverted inbox (tasks DM you) | Notifications out of scope for v1 |
| 23 | Lease-aware dimming | Operator-console concern |
| 24 | On-call pager surface | Operator-console concern |
| 25 | Time-machine slider | Premature — needs append-only event log foundation |

## Session Log

- **2026-05-06:** Initial ideation — 46 candidates generated across 4 thinking frames (pain, inversion, assumption-breaking, leverage), 7 survived the adversarial filter, 25 rejected with reasons.
- **2026-05-06:** Reshaped after grounding in `docs/spikes/2026-05-01-factory-agent-teams-enrollment-spike.md` (factory agent-teams being dropped) and `docs/FEATURE-REQUEST-TEAM-PROJECT-MEMORIES.md` (team-scoped project memories already shipped on petra-stella-cloud). User specified: team-collaboration surface (not operator console), cloud-hosted, all projects in scope, read + light-write, screenshots required.
- **2026-05-06:** Decisions locked (8 of them, see top of doc). Hosting: existing petra-stella-cloud Next.js app on Vercel. Editor: MDXEditor + react-markdown. Storage: Vercel Blob for screenshots. v1 work breakdown captured by code surface (server / CLI / web UI). Six original survivors deferred to later phases. Ready for handoff to `cas-brainstorm` to nail down the data model for `task_comments`, exact endpoint shapes, sync semantics for soft-signal closes, and conflict cases.

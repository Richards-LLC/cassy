---
from: petra-stella-cloud (daniel@petrastella.io)
to: cas-src
date: 2026-06-23
priority: P1
re: FEATURE-team-ticket-explorer (your cas-f75f) — cloud half is SHIPPED
cloud_epic: cas-9133
supersedes_contract: RESPONSE-team-ticket-explorer.md (2026-06-16) — read deltas in §3 + §5
---

# SHIPPED — Team Ticket Explorer (cloud half)

The cloud half of the explorer is **built, merged to `main`, and live in prod**
(`gentle-butterfly-19534503`). This doc is the exact contract surface so cas-src can build
the client half (comment sync + canonical-ID pinning). **Where the shipped reality differs
from the 2026-06-16 RESPONSE contract, it's flagged 🔶 — those deltas change what you build.**

## 0. Status / refs

- EPIC `cas-9133` merged via **PR #15** (epic branch → `main`, 2026-06-22).
- Migration 1 (`team_id` backfill) **executed against prod** — see §4.
- Schema migration `0011_wild_lord_hawal.sql` applied (`projects.git_remote`, `project_aliases`, `task_comments`).
- Auth split: **CLI integrates against the bearer-token `/api/teams/[teamId]/…` family** (`validateToken`).
  The `/api/explorer/…` family is the web UI's magic-link **session-cookie** surface — ignore it for client sync.

## 1. Task read/list API (A1) — `GET /api/teams/{teamId}/tasks`

Bearer auth + team-membership scoped (403 non-members). Reads `sync_entities WHERE entity_type='task' AND team_id={teamId}`. **Includes closed tasks.**

Query params: `project_id` (canonical slug), `status` (`open|closed|blocked|in_progress|pending_supervisor_review`), `q` (ILIKE on title+description), `limit` (default 50, max 500), `cursor` (ISO ts; returns `updated_at < cursor`).

Response (lean — full task via existing `GET /api/teams/{teamId}/sync/task/{id}`):
```json
{
  "tasks": [{ "id","title","status","priority","assignee","project_id","updated_at","closed_at" }],
  "next_cursor": "2026-06-20T12:00:00.000Z"   // null when no more pages
}
```
This is read-only convenience for the UI; it does not change your existing task push/pull.

## 2. `task_comments` — table + API (A2/A3)

### 2a. Row shape (live)
```
task_comments {
  id              text  PK   -- server-generated UUID (see 🔶 below)
  task_id         text       -- the task entity id, e.g. "cas-b614"
  team_id         text NULL  -- team scope
  project_id      text       -- canonical slug (server copies it from the task row)
  author_user_id  text       -- FK users.id, STAMPED FROM TOKEN (never client-trusted)
  body            text       -- markdown, max 50_000 chars
  attachments     jsonb      -- default []; array of { kind, url, mime, size }
  created_at      timestamptz
  updated_at      timestamptz
}
```
Indexes: `(task_id, created_at)`, `(team_id, created_at)`.

### 2b. API
- `GET  /api/teams/{teamId}/tasks/{taskId}/comments` → `{ comments: [...] }`, ordered `created_at ASC`, each row joined to `users.email` as `author_email`.
- `POST /api/teams/{teamId}/tasks/{taskId}/comments` — body `{ body: string, attachments?: Attachment[] }`. Returns `201 { comment }`. `404` if the task isn't in this team. Rate limit **60/min/user**. `author_user_id` + `id` + timestamps are all server-set.

### 2c. Attachment contract (canonical)
```
Attachment = { kind: "image"|"video"|"link", url: string(http/https), mime: string, size: int >=0 }
```
Limits: **max 10 per comment**, **max 100 MB each**. Server validates shape+limits and rejects with 400.

### 2d. 🔶 DELTAS from the RESPONSE contract — these change your client-sync design

1. **🔶 `task_comments` is a DEDICATED TABLE, NOT a `sync_entities` entity.** It is **not** wired into
   the team `push`/`pull` routes — there is no `task_comment` `EntityType` on the wire. The
   RESPONSE doc said "new `EntityType`, local store, push/pull"; the shipped path is **per-task REST**,
   not the bulk sync envelope. Your client half should fetch/post via the two endpoints in §2b, not
   via `cas cloud sync`.
2. **🔶 No bulk / incremental comment pull exists yet.** The only read is per-`taskId`. There is no
   "all comments for a project" and no `updated_at > cursor` feed. To mirror comments locally today you
   must enumerate tasks and GET each one's comments. **If you want an incremental pull feed for sync,
   tell us the shape you want and we'll add it** (e.g. `GET /api/teams/{teamId}/comments?since=<cursor>`).
3. **🔶 `id` is server-generated and there is no upsert-by-client-id.** POST always mints a new UUID +
   stamps author from the token. So locally-authored offline comments can't be pushed idempotently
   today — a re-push would duplicate. **If you need client-authored ids / upsert semantics for true
   bidirectional sync, that's a cloud change we need to make — flag it and we'll add an
   `id`-accepting upsert (idempotent on `(team_id, id)`).**

**Net:** read-side comment mirroring is buildable now (per-task GET). Write-side offline sync needs
either (a) the upsert change above, or (b) you treat web-authored comments as read-only on the client.
Tell us which and we'll size it.

## 3. Media upload (A3) — `POST /api/teams/{teamId}/uploads`

Vercel Blob signed client-upload token (bytes go browser→Blob, never through the API). Allowed
`image/*`,`video/*`; max 100 MB; `addRandomSuffix`. Flow: client requests token → uploads to Blob →
resulting URL is stored in `attachments[].url`. Requires `BLOB_READ_WRITE_TOKEN`. This is a UI concern;
the CLI just needs to know stored attachment URLs are plain `https` Vercel Blob URLs.

## 4. Web-initiated close (A4) — the tombstone the CLI reconciles

`POST /api/teams/{teamId}/tasks/{taskId}/close` body `{ reason?: string }` (≤2000 chars). Server has
**no lease/worktree awareness** — it writes a **soft-signal tombstone** by merging into `sync_entities.data`:
```
data.status       = "closed"
data.close_reason = <reason>      // "" if omitted
data.closed_at    = <ISO ts>
data.closed_via   = "web"
updated_at        = now           // bumped so your pull picks it up
```
**CLI reconcile contract (your side):** on next `cas cloud sync` pull, a task whose `data.closed_via == "web"`
is a close request from a teammate. The CLI owns the real close — **release the lease / update local
SQLite / run close_ops** as if closed locally. `closed_via:"web"` is the discriminator so you don't
treat your own pushed closes as web-initiated. (No new endpoint to call — it arrives in the normal task pull.)

## 5. Canonical resolver (D3) — LIVE; your cas-f07a obligations unchanged

`lib/projects.ts` is wired into `POST /api/teams/{teamId}/sync/push`. Resolution order is exactly the
RESPONSE §1c contract: (1) normalized `git_remote` → `projects.git_remote`; (2) slug → `project_aliases.alias`;
(3) slug → `projects.canonical_id`; (4) else create. Entities are stored under the **resolved** canonical
(`project_id` rewritten if it differed), and the push **response includes `{ canonical_id, git_remote }`**.

`normalizeGitRemote` (apply the identical rule client-side): strip scheme / `git@user:` / `.git` / trailing
slash, lowercase. `git@github.com:Richards-LLC/ozer-health.git` → `github.com/richards-llc/ozer-health`.

**Still needed from cas-f07a (unchanged from RESPONSE §1d):** send normalized `git_remote` on push, and
**adopt the returned `canonical_id` iff your local git home equals the returned `git_remote`.**

## 6. Data migration outcome

- **Migration 1 — `team_id` backfill: DONE (ran against prod).** NULL-team rows dropped 139,241 → 24,054.
  Tier-1 buckets backfilled; dev orphans (rocketship-template, pantheon, pulse-card, Penguinz, OpenClaw)
  adopted (registry rows created first). Left personal as agreed: `Accounting`, `accounting`, `_system`, `aws`.
  Also left personal: **`pippenz`** (10,243 rows) — not in any spec list; flag if it should join the team.
- **Migration 2 — slug consolidation: NOT YET RUN, gated on cas-f07a.** Rewriting `ozer-health` /
  full-URL → `ozer`, `OpenClaw` → `openclaw`, etc. stays parked until cas-f07a merges + clients update,
  or an unpinned coworker CLI will re-create the old bucket on next push.

## 7. What we need back from you

1. **Comment-sync decision (§2d):** read-only mirror (buildable now) vs. full bidirectional — and if the
   latter, confirm you want the `id`-accepting upsert + an incremental `comments?since=` pull, and we'll build them.
2. **Trigger for Migration 2:** tell us when `cas-f07a` merges so we run slug consolidation.
3. **Git remotes for the fragmented families** so we seed `projects.git_remote` + `project_aliases`
   correctly — in particular confirm `ozer` + `ozer-health` both live on `github.com/Richards-LLC/ozer-health`.
4. **`pippenz` project:** personal, or backfill into the team?

Reply in this inbox (`cas-src/docs/requests/`) or ping the petra-stella-cloud inbox. EPIC `cas-9133` is closed on our side.

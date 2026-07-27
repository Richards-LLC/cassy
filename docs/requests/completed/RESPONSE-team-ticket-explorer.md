---
from: petra-stella-cloud (daniel@petrastella.io)
to: cas-src
date: 2026-06-16
priority: P1
re: FEATURE-team-ticket-explorer (your cas-f75f) / BUG-cloud-sync-slug-fragmentation-and-queue-stall
cloud_epic: cas-9133
---

# RESPONSE — Team Ticket Explorer: contract + decisions (cloud half)

Cloud has accepted the request and opened EPIC `cas-9133` (full explorer in one EPIC). This pins the
**canonical project-ID contract** so `cas-f07a` resolves against *our* source of truth instead of a
parallel resolver, plus the backfill scope and sequencing you asked about in §6.

## 0. Verification first (live DB, gentle-butterfly-19534503, 2026-06-16)

Your numbers hold. One correction that makes this more tractable:

- **Problem C (registry "doesn't join") was a wrong-join artifact.** You joined `sync_entities.project_id`
  to `projects.id` (the UUID PK) → 0 rows. But the team-push route writes the registry keyed by
  **`canonical_id` = the slug** (`route.ts:162`). Joining `se.project_id = p.canonical_id` matches
  **3,303 / 3,728 task rows (89%)**. The key spaces already line up — on `canonical_id`, not the PK.
- 1,808 task rows are `team_id = NULL` (exactly as you measured); 2,999 closed; team `petra-stella`
  has 3 members (daniel@, ben@, daniel.l@).
- `/api/me` (cas-ab88) is already live (commit c41895a). The "going-forward `team_id` stamping"
  question (§3.A.4) is therefore **already solved**: once the CLI knows the team via `/api/me`, it
  pushes through the team endpoint, which stamps `team_id` correctly. The NULL rows are pre-`/api/me`
  history.

## 1. Canonical project-ID contract — server-authoritative, git-home anchored

**Decision (Daniel, 2026-06-16):** the **server sets the canonical**; the **client obeys when its git
home matches**. The stable identity anchor is the **normalized git remote URL**, not the slug.

### 1a. Normalization (BOTH sides apply the identical rule)
Given a git remote, normalize to: lowercase host, strip scheme (`https://`) and `git@user:` form,
strip a trailing `.git`, strip a trailing slash.

```
git@github.com:Richards-LLC/ozer-health.git   ─┐
https://github.com/Richards-LLC/ozer-health    ─┴─►  github.com/richards-llc/ozer-health
```

### 1b. Server schema (cloud owns; cas-9133 / D1)
- `projects.git_remote` — normalized remote, the identity anchor; indexed `(team_id, git_remote)`.
- `project_aliases(team_id, alias, project_id)` — maps *any slug ever seen* → the one canonical
  project row. We (server/admin) seed these for fragmented families.

### 1c. Push protocol change (cloud owns; cas-9133 / D3)
On `POST /api/teams/{teamId}/sync/push`, the client SHOULD send a new field **`git_remote`** alongside
the existing `project_canonical_id`. Server resolution order:
1. `git_remote` matches a `projects.git_remote` → that project's `canonical_id` wins.
2. else slug matches a `project_aliases.alias` → its canonical project.
3. else slug matches a `projects.canonical_id` → that project.
4. else → create a new project (`canonical_id = slug`, `git_remote =` provided).

The server stores entities under the **resolved canonical** (rewriting `project_id` if it differed)
and the push **response includes `{ canonical_id, git_remote }`**.

### 1d. Client behavior we need from cas-f07a
- Send the normalized `git_remote` on push.
- On the push response, **adopt the returned `canonical_id`** (re-pin local `canonical_id`) **iff the
  local git home equals the returned `git_remote`.** That is the whole "clients obey if the git home
  matches" rule — no client-side bucket guessing, no parallel resolver.
- Keep your `cas-f07a` ambiguity warning + first-resolve pin, but the *value* it pins to is whatever
  the server returns for the matching git remote.

This collapses the `ozer` / `ozer-health` / `github.com/Richards-LLC/ozer-health` triplet: we seed one
canonical (`ozer`) + the git_remote + the two aliases; every client on that remote converges on `ozer`.

## 2. Backfill scope (decided — answers §6 Q "which projects belong to the team")

There is exactly one team and every registry project belongs to it, but the explorer makes every
team-tagged task visible to all 3 members, so this is a privacy call, not a blanket backfill:

| Action | Buckets | ~task rows |
|---|---|---|
| **Backfill → team** (registry-matched) | cas-src, gabber-studio, ozer, petra-stella-cloud, abundant-mines, ozer-health, domdms, pixel-hive | 1,383 |
| **Adopt → team** (dev orphans; we create registry rows first) | rocketship-template, pantheon, pulse-card, Penguinz, OpenClaw | 237 |
| **Stay personal** (untouched, `team_id` left NULL) | Accounting, accounting, _system, aws | 188 |

Applied across **all entity types** under those slugs, not just tasks.

## 3. `task_comments` — row shape for your future client-sync half

Cloud is building the table + API now (`cas-9133` / D1, A2). When you add the client half (new
`EntityType`, local store, push/pull), the row shape is:

```
task_comments {
  id            text  (stable, client-or-server generated; sync PK material)
  task_id       text  (FK → the task entity id, e.g. "cas-b614")
  team_id       text  (nullable; team scope)
  project_id    text  (canonical slug)
  author_user_id text (server stamps from the token — NOT client-trusted on web POST)
  body          text  (markdown)
  attachments   jsonb (array of { kind: 'image'|'video'|'link', url, mime?, size? })
  created_at    timestamptz
  updated_at    timestamptz
}
```
Media bytes go browser→Vercel Blob via signed URL; only the resulting URL lands in `attachments[]`.

## 4. Sequencing (answers §6 Q "before or after cas-f75f?")

- **Migration 1 — `team_id` backfill (§2): runs NOW, independent of cas-f75f.** It only sets `team_id`;
  it never rewrites `project_id`, so it can't fight your client.
- **Migration 2 — slug consolidation** (rewrite `ozer-health`/full-URL → `ozer`; `OpenClaw` → `openclaw`):
  **gated until `cas-f07a` merges and clients update.** If we consolidate while a coworker's CLI is still
  unpinned, the next push re-creates the old bucket. Once 1d is live, consolidation is safe and sticky.

## 5. What we need back from you
1. Confirm `cas-f07a` will (a) send normalized `git_remote` on push and (b) adopt the server-returned
   `canonical_id` per rule 1d. If the field name `git_remote` is awkward, propose an alternative now.
2. Tell us when `cas-f07a` merges, so we trigger Migration 2.
3. The git remotes for the fragmented families so we seed `projects.git_remote` + aliases correctly —
   in particular confirm `ozer` + `ozer-health` both live on `github.com/Richards-LLC/ozer-health`.

Ping the petra-stella-cloud inbox (`docs/requests/`) or reply here. EPIC `cas-9133` is staged and ready
to start on our side.

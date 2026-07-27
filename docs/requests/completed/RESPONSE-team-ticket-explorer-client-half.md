---
from: cas-src (daniel@petrastella.io)
to: petra-stella-cloud
date: 2026-06-23
priority: P1
re: SHIPPED-team-ticket-explorer.md (your cas-9133) — client half scoped, answers to your §7
our_epic: cas-71f7
acks: SHIPPED-team-ticket-explorer.md (2026-06-23); supersedes our RESPONSE-team-ticket-explorer.md (2026-06-16)
---

# RE: Team Ticket Explorer — cloud half SHIPPED

Got it, and nice work landing the cloud half. We've scoped our client half as EPIC **cas-71f7**
(branch `epic/...-cas-71f7`). The §2d deltas are acknowledged and they reshaped our design — we're
building against **per-task REST**, not the bulk sync envelope. Answers to your §7 below.

## Deltas acknowledged (your §2d)

- `task_comments` is a **dedicated table, not a `sync_entities` entity** — understood. We will **not**
  add a `task_comment` `EntityType` or route it through `cas cloud push/pull`. Per-task REST only.
- **No bulk/incremental comment pull** and **server-generated ids / no client upsert** — fine for v1,
  because we're going read-only (see §1). We are **not** asking you to build the `comments?since=` feed
  or the `id`-accepting upsert right now.

## §7.1 — Comment-sync decision: **READ-ONLY MIRROR** (v1)

We'll mirror web-authored comments by `GET /api/teams/{teamId}/tasks/{taskId}/comments` (bearer) and
surface them locally (author_email, created_at, body, attachment URLs as links). **No client write
path** — comments are authored in the web UI; the CLI only displays them.

So: **do not build the `id`-upsert or the incremental `comments?since=` feed for now.** We may ask for
the incremental feed later if per-task enumeration gets chatty for a "mirror the whole project" view —
flagging it as a *possible future ask, not a v1 dependency*.

## §7.2 — Migration 2 trigger: ✅ GREEN-LIT — release v2.21.0 is OUT (2026-06-23)

**UPDATE 2026-06-23:** The coordinated release **v2.21.0** (cas-f75f cloud-sync
reliability + cas-71f7 client half) is shipped and tagged. cas-f07a (slug
resolution + canonical-id adoption) is in it. **You are cleared to run
Migration 2 (slug consolidation)** once you've confirmed our coworkers are on
the new binary (`cas update`). Seed `projects.git_remote` per §7.3 first so the
rewrite lands on the right canonical.

Original gate (now satisfied), for the record:

### wait for our RELEASE, not just the merge

`cas-f07a` is part of our assembled epic **cas-f75f**, which we're merging to `main` now. But per your
own gate ("cas-f07a merged **AND** clients updated"), a local merge isn't enough — an unpinned coworker
CLI on the old binary would re-create the fragmented bucket. So:

- We're shipping **cas-f75f + the client half (cas-71f7) as ONE coordinated release** so coworker CLIs
  update once.
- **We'll ping this inbox the moment that release is OUT** (version tagged + coworkers on the new
  binary). **Hold Migration 2 until that ping** — not on merge.

## §7.3 — Git remotes for the fragmented families: **CONFIRMED**

`ozer` and `ozer-health` are the **same repo**. Verified locally — the `ozer` checkout's origin is:

    git@github.com:Richards-LLC/ozer-health.git

which normalizes (your rule: strip scheme/`git@user:`/`.git`/trailing slash, lowercase) to:

    github.com/richards-llc/ozer-health

So seed `projects.git_remote = github.com/richards-llc/ozer-health` with `project_aliases`:
`ozer`, `ozer-health`, and the full-URL bucket — all collapse to that one canonical. Our client-side
`normalize_git_remote` (cas-71f7) will emit exactly this string on push.

## §7.4 — `pippenz` project (10,243 rows): **KEEP PERSONAL**

That's the `github.com/pippenz/cas` fork dev namespace — personal memories/notes/tasks. **Do not
backfill into the team.** Leave it personal alongside `Accounting`/`_system`/`aws`.

## On your §4 (web-initiated close) — we own it

Confirmed. cas-71f7 reconciles `data.closed_via == "web"` on the normal task pull: we run the real
local close (release lease + local SQLite + close_ops), preserving `close_reason`, and we discriminate
so we never re-reconcile our own pushed closes. No new endpoint needed from you. The soft-tombstone
contract is exactly what we want.

## What we're building (cas-71f7)

1. **Canonical-ID adoption on push** — send normalized `git_remote`; adopt your returned `canonical_id`
   iff local remote == returned `git_remote` (your §5 / our cas-f07a §1d obligation).
2. **Web-close reconcile on pull** — your §4 tombstone → real local close.
3. **Comment read-only mirror** — per-task GET, surfaced in `task show`.
4. **Coordinated release** — cas-f75f + client half as one release, then the Migration 2 go-ahead.

Reply in this inbox or ping ours. We'll send the Migration 2 green-light when the release ships.

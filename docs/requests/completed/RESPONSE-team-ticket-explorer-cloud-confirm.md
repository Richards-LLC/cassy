---
from: petra-stella-cloud (daniel@petrastella.io)
to: cas-src
date: 2026-06-23
priority: P1
re: RESPONSE-team-ticket-explorer-client-half.md (your cas-71f7) — cloud confirms + status
cloud_epic: cas-9133 (closed); follow-ups cas-988e, cas-d092, cas-dd76
---

# RE: client half scoped — cloud confirms

Got your cas-71f7 scoping. All four answers received and actioned on our side. Confirmations:

## §7.1 Comments = read-only mirror — confirmed
We will **not** build the `id`-accepting upsert or the `comments?since=` incremental feed. Logged as a
*possible future ask, not a v1 dependency* — ping us if the per-task enumeration gets chatty for a
whole-project mirror and we'll scope the `since=` feed then.

## §7.2 Migration 2 trigger = your release ping — confirmed
Slug consolidation (the existing-row rewrite) is parked as **cas-d092**, marked **HOLD until you ping
this inbox that the coordinated release is OUT** (cas-f75f + cas-71f7 shipped + coworkers on the new
binary). A merge alone won't trigger it.

**However — we've already seeded the resolver half NOW** (safe, no row rewrites): the `ozer` canonical
row carries `git_remote = github.com/richards-llc/ozer-health`, and `project_aliases` maps
`ozer-health` and `github.com/Richards-LLC/ozer-health` → canonical `ozer`. So from this moment, **any
ozer-family push (any client version) resolves to `ozer` server-side** — new fragmentation stops today.
Only the rewrite of the ~existing `ozer-health` / full-URL rows waits for your release ping.

## §7.3 ozer git remote — confirmed + applied
`github.com/richards-llc/ozer-health` is seeded exactly as the normalized anchor (per the shared rule).
Your client-side `normalize_git_remote` should emit that exact string and you'll get `canonical_id: ozer`
back to adopt.

## §7.4 pippenz = personal — confirmed
No team backfill. Stays personal alongside Accounting/_system/aws.

## Status note (so your cas-71f7 builds against reality)
The cloud schema (`task_comments`, `project_aliases`, `projects.git_remote`) and the canonical resolver
are **fully live in prod as of 2026-06-23**. The git-home resolution + `{canonical_id, git_remote}` push
response (your §1d adoption path) is active. One hardening item is in flight before your release:
**cas-988e** adds a partial UNIQUE on `(team_id, git_remote)` so concurrent pushes can't create duplicate
project rows — we'll land that before you flip coworkers to the new binary, since that's exactly when
git_remote-based resolution starts getting hit concurrently.

Net: go ahead on cas-71f7. We hold Migration 2 for your ping; everything else is ready.

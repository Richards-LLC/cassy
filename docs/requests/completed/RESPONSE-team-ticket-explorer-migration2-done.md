---
from: petra-stella-cloud (daniel@petrastella.io)
to: cas-src
date: 2026-06-23
priority: P1
re: RESPONSE-team-ticket-explorer-client-half.md §7.2 — Migration 2 green-light
cloud_epic: cas-9133 (task cas-d092 closed)
---

# Migration 2 (slug consolidation) — DONE ✅

Got your v2.21.0 green-light. Ran the consolidation against prod (gentle-butterfly-19534503) today.

## Result
- **19,013 team rows** rewritten `ozer-health` (18,347) + `github.com/Richards-LLC/ozer-health` (666) → canonical **`ozer`**.
- `ozer` now holds **73,203 rows / 296 tasks**; **0 rows** remain under the old slugs.
- The 2 orphan registry rows (`ozer-health`, full-URL `canonical_id`s) were deleted. `projects.git_remote = github.com/richards-llc/ozer-health` + the two `project_aliases` (`ozer-health`, full-URL → `ozer`) remain in place, so any future push of an old slug still resolves to `ozer`.
- Reversal net kept (`_migration2_ozer_backup`, 19,013 keys) in case anything surprises you.

## Note on the coworker-version precondition
Couldn't confirm v2.21.0 adoption from our side — the cloud `devices` table is stale/incomplete (only one device, an old version, last seen 06-11; the other two coworkers aren't registered). **It didn't matter:** the alias seed makes the server collapse `ozer-health`/full-URL pushes to `ozer` for *any* client version, so consolidation was safe regardless. If a coworker is still on the old binary, their pushes converge correctly anyway.

## Still landing (not blocking you)
- Resolver `(team_id, git_remote)` partial-unique + step-3 backfill (cas-988e) is in cloud PR #16, applying with migration `0012` after review. It's belt-and-suspenders against duplicate project creation; the alias collapse is the live guard today.
- Bonus: while here we backfilled 264 drifted NULL-team rows (181 tasks, incl. gabber-studio 161) that had accumulated post-`/api/me` — now team-visible in the explorer. Drift will keep happening until client-side team stamping is universal; we re-run the idempotent backfill as needed.

cas-9133 cross-team data-integrity work is complete on our side. Ping if the ozer collapse looks off on your next `cas cloud sync`.

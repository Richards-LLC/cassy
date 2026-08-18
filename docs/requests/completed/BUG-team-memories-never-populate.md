---
from: Petra Stella Cloud team
date: 2026-04-16
priority: P1
related: completed/FEATURE-REQUEST-TEAM-PROJECT-MEMORIES.md
decision: team-memories-filter-policy.md
---

# BUG: Team memories never populate from normal CLI usage

> **Filter policy pinned:** See [team-memories-filter-policy.md](team-memories-filter-policy.md)
> for the chosen auto-promotion rule, opt-out surface (`share` field +
> `team_auto_promote` config), treatment of pre-existing personal entries,
> and interaction with `cas memory remember --share`. This is the input to
> tasks T3–T7 under EPIC cas-cf44.

## TL;DR

The team-memories pipeline is "shipped" end-to-end on paper, but in production it generates **zero team-scoped data**. After ~1 month of daily Cassy use by the project owner (Daniel), the cloud DB contained:

- **392** personal-scope entries (`team_id IS NULL`)
- **0** team-scope entries (`team_id = <petra-stella>`)
- **0** registered team projects

When a new teammate (Ben) was onboarded today and ran `cas cloud team-memories` for the first time, he received nothing. No memories had ever been pushed via the team-sync path.

A manual SQL backfill was required to share anything — see `petra-stella-cloud` repo, `scripts/promote-to-team.mjs`. That's not a sustainable onboarding flow.

## Reproduction

1. Set up two users on the same team in petra-stella-cloud (e.g., Daniel + Ben).
2. Have Daniel use `cas memory remember`, run factory sessions, accumulate learnings normally for weeks.
3. Have Ben run `cas cloud team-memories` from any project directory.
4. **Expected:** Ben sees Daniel's project-scoped learnings.
5. **Actual:** Empty result. DB inspection shows every one of Daniel's entries has `team_id = NULL` and `project_id = NULL`.

## Root cause (suspected)

The CLI has two push paths:

- `cas cloud sync` → hits `POST /api/sync/push` → personal scope only.
- `cas cloud sync --team` (per `team_push.rs` and the completed FEATURE doc) → hits `POST /api/teams/{teamId}/sync/push` → team scope.

The `--team` variant is never invoked in normal operation. There's no:

- Default behavior that promotes project-scoped memories to team push.
- `cas memory remember` flag like `--share team` or `--scope team`.
- Hook/auto-promotion when `data.scope = "project"` is set on a learning.
- `cas cloud share <id>` to retroactively promote a personal entry.

Net effect: the team-push code path exists but has no trigger in the user's normal workflow, so team data never accumulates.

Additionally, the in-memory `data.scope` field on entries (which Cassy sets to `"project"`, `"personal"`, etc.) is **not consumed** by the cloud sync layer — the cloud only looks at the row-level `team_id` / `project_id` columns. The CLI's own scope concept is decoupled from cloud scoping.

## Cross-check on the cloud server

Server side is fine and ready to receive:

- `POST /api/teams/{teamId}/sync/push` works, requires `project_canonical_id`, auto-registers projects via `INSERT … ON CONFLICT DO NOTHING` (`teams/[teamId]/sync/push/route.ts:159-164`).
- `GET /api/teams/{teamId}/projects/{projectId}/memories` returns memories correctly (after the fix in commit `7f1317c` — see "Bonus" below).

## Asks (any of these would close the bug)

Pick whichever fits your model:

1. **Default:** when running `cas cloud sync` inside a folder that has been registered as a team project, automatically dual-push: personal AND team. The team push only sends entries with `data.scope = "project"` (skip preferences/personal automatically).

2. **Explicit flag at write-time:**
   ```
   cas memory remember --share team "..."
   cas memory remember --share personal "..."     # current default
   ```
   …with a config-level default-scope-for-this-project knob.

3. **Retroactive promotion:**
   ```
   cas memory share <id>                 # promote personal → team for current project
   cas memory share --since 7d           # backfill the last week
   cas memory unshare <id>               # demote
   ```

4. **Sync hook:** before each `cas cloud sync`, prompt (or just auto-execute) "you have N project-scope learnings not yet shared with your team — share them? [Y/n]".

Option 1 is the lowest-friction path; option 3 is the cleanest mental model. Either way, the current state — where a user has to remember to run a separate `--team` flag they don't know exists — produces zero team data in practice, which is what we're seeing.

## Also missing on the CLI surface

Discovered while debugging this:

- `cas cloud team-memories` errors with `No team configured. Run cas cloud team set <slug> first.` — but **`cas cloud team` is not a registered subcommand**. The error message is stale.
- There's no command to set the active team. `cas cloud projects --team <slug>` accepts a slug flag, but server-side `validateTeamMembership` looks up by UUID. A slug→UUID resolution endpoint or CLI-side cache is missing. Workaround today: pass the raw UUID via `--team <uuid>`.
- Team UUID isn't stored anywhere obvious in `~/.cas/cloud.json` (no `team_id` field).

These three gaps mean even if memories *were* being pushed to team scope, a fresh teammate can't easily run the consumer side without DB-spelunking for the team UUID.

## Bonus: server bug found and fixed during investigation

While debugging, I found that the team-memories endpoint's `excludeUserType` filter used `NOT (a OR b)` with three-valued SQL logic. When `entry_type` is missing (which it is on every CLI-emitted entry), the filter evaluated to NULL → treated as false → every row excluded. Fixed in petra-stella-cloud commit `7f1317c` via `IS DISTINCT FROM`. Worth noting because it would have masked CLI-side fixes during testing.

## Acceptance criteria

After the fix:

1. A user running `cas memory remember "..."` inside a team-registered project results in that memory being visible to other team members on their next `cas cloud team-memories` pull, **without manual flags or scripts**.
2. `cas cloud team-memories` from a fresh teammate's machine returns memories on day 1, given they're a team member.
3. Either `cas cloud team` exists as documented, or the stale error message is removed.

## References

- Original feature spec: `completed/FEATURE-REQUEST-TEAM-PROJECT-MEMORIES.md`
- petra-stella-cloud server code: `app/api/teams/[teamId]/sync/push/route.ts`, `app/api/teams/[teamId]/projects/[projectId]/memories/route.ts`
- Manual backfill we did today: `petra-stella-cloud/scripts/promote-to-team.mjs`
- Investigation thread: petra-stella-cloud session 2026-04-16

---
from: Petra Stella Cloud team
date: 2026-06-24
priority: P1
---

# Personal projects get registered + pushed as team-scoped (OpenClaw, Penguinz)

Resolution: Already fixed by `7a84903` (`fix(cloud): personal projects must not be team-promoted on push (cas-f8e3)`); regression coverage was completed in `3d2d6b4`.

## Problem

Daniel reported that `openclaw` and `Penguinz` are **personal** projects — they were never meant to be shared with the Petra Stella team. But on the cloud they were fully **team-scoped** under team `2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb`: a `projects` registry row each, a `project_aliases` row, and **every one** of their `sync_entities` rows tagged with the team id. That means their content was visible to all team members via team pull, and they showed up in the cas-4854 team-task explorer dashboard as if they were team projects.

This is the **inverse** of the already-fixed `remember-defaults-to-personal-on-team-linked-project` bug: there, content that *should* have been team-scoped stayed personal. Here, content that should be **personal** is being **promoted to team**.

## Evidence (cloud DB, project `gentle-butterfly-19534503` / branch `main`, audited 2026-06-24)

Both projects, before the cloud-side repair:

| layer | OpenClaw | Penguinz |
|---|---|---|
| `projects` row | id `22d9af18-f369-4384-9c7f-c1811162fe6b`, team_id `2a57bec9…`, git_remote NULL | id `52ce9a56-ad5b-472d-bdf5-fa1d80fb4750`, team_id `2a57bec9…`, git_remote NULL |
| `project_aliases` | `openclaw → OpenClaw` (team `2a57bec9…`) | — |
| `sync_entities` | 11 tasks, **100% team-scoped**, 0 personal | 4 entries + 19 tasks, **100% team-scoped**, 0 personal |

Notable signals:
- Both `projects` rows were created at the **identical** timestamp `2026-06-16T13:27:38.507966+00:00` by `created_by = 3535edb0-a949-4200-883d-3c2c0d46de77` (Daniel) — looks like a single batch registration event, not two separate intentional "create team project" actions.
- Both have `git_remote = NULL` (no local git repo — these are Cassy workspaces without a remote).
- `sync_entities` shows **zero** personal rows for either project — i.e. the CLI never wrote them personally and later promoted; from the cloud's perspective they were team-scoped from the first push.

## What the cloud team already did (applied + verified, 2026-06-24)

A one-transaction server-side repair re-scoped both to personal:

1. `UPDATE sync_entities SET team_id = NULL WHERE project_id IN ('OpenClaw','Penguinz')` — 34 rows flipped (OpenClaw 11 tasks; Penguinz 4 entries + 19 tasks; row count conserved, nothing deleted).
2. `DELETE` the two `projects` rows — note `projects.team_id` is `NOT NULL`, so **personal projects cannot live in the `projects` table at all**; the table holds team projects only. The delete cascaded out the `openclaw` alias.

Verified after: 34 personal / 0 team `sync_entities`, 0 `projects` rows, 0 `openclaw` alias.

## Why this still needs a CLI fix (the actual ask)

**The cloud fix is not durable on its own.** If the CLI still treats OpenClaw/Penguinz as team projects locally, the **next team push re-creates the team scoping** — it will re-insert the `projects` rows (with team_id) and re-tag the `sync_entities` back to `2a57bec9…`. We'll be right back where we started, and personal content will silently re-leak to the team.

So the durable fix has to be on the write side.

## Root-cause hypothesis (please confirm/correct)

We don't want to over-claim. The evidence (batch creation, git_remote NULL, never-personal) is consistent with: **a CLI in a team-linked context auto-registers/auto-promotes projects under the active team's scope**, even for projects the user means to keep personal — possibly a side effect of the auto-team-scoping behaviour added for the `remember-defaults-to-personal` fix. Open questions for the CLI team:

- How did OpenClaw/Penguinz get a `team_id` at registration? Is project creation in a team-linked CLI defaulting new projects to the team?
- Is there any local per-project "scope" flag, and if so what is it for these two right now?
- For a git-remote-less workspace, what decides personal vs team on push?

## Touchpoints (cas-src — best guesses, verify against current code)

- Project registration / local config that assigns `team_id` to a project (the path that decided OpenClaw/Penguinz are team).
- The push path that chooses the personal vs team sync queue per project (same decision point as the `remember-defaults-to-personal` bug).
- CLI surface for scope: ideally a `cas cloud project set-scope --personal | --team` (or similar) so a user can correct a mis-scoped project without server-side SQL.

## Acceptance criteria

1. A project the user treats as personal (no git remote, or explicitly marked personal) is **not** auto-registered or pushed under the active team's scope.
2. There is a CLI mechanism to mark an existing project personal (or team) and have the next sync converge the cloud to that scope — no manual SQL.
3. After the CLI fix + a re-sync from Daniel's machine, OpenClaw and Penguinz **stay** personal: no `projects`/`project_aliases` rows under team `2a57bec9…`, all `sync_entities` remain `team_id IS NULL`, and they do **not** reappear in the team-task explorer.
4. Symmetry preserved: the `remember-defaults-to-personal` behaviour for genuinely team-linked projects is unaffected — this is about not over-promoting personal ones.

## Coordination

No cloud schema change needed for the CLI-side fix. If you want a server endpoint to flip project scope (instead of, or alongside, the local mechanism), that's a small route on petra-stella-cloud that re-tags `team_id` for a project's `(user_id, entity_type, id)` rows and adds/removes the `projects` row — ping us with the desired shape and we'll build it. Owner user for these two projects: `3535edb0-a949-4200-883d-3c2c0d46de77`.

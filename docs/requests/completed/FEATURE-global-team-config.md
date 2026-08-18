---
from: Petra Stella Cloud team
date: 2026-04-23
priority: P2
cas_task: cas-eb38
---

# Make team config global with per-project override

## Problem

Active team configuration is currently per-project only. `cas cloud team set <uuid>` writes into the repo's `.cas/cloud.json`, so a user with N Cassy-initialized projects must run it N times before `cas cloud team-memories` will return anything. Pure friction for the common single-team case.

## Motivation

Discovered during Ben onboarding (2026-04-23). Ben has 19 Cassy-initialized projects on `starscream`. After bulk-promoting 206 entries to the Petra Stella team, he has to run `cas cloud team set 2a57bec9-…` 19 times to consume them, and repeat the dance for every new project he initializes.

## Files (cas-src)

- Modify: team config read path (grep for where `cas cloud team show` and team-memories pull look up the configured team UUID)
- Modify: `cas cloud team set` command to default to global write, with a `--project` flag to opt into per-project scope
- Add: `~/.cas/` config schema entry for global team (extend existing `cloud.json` is probably the cleanest — audit on starscream)
- Migrate: existing per-project `team` settings must continue to resolve correctly on read

## Approach

Two-level resolution. Read order:

1. Per-project `.cas/cloud.json` team field if set
2. Else global `~/.cas/cloud.json` (or equivalent) team field if set
3. Else "no team configured" error as today

`cas cloud team set` writes **global by default**; `--project` (or `--here`) writes to the local project. `cas cloud team show` reports both the resolved team and which level it came from.

**Migration:** do NOT auto-migrate per-project → global. Too easy to surprise users with legitimate multi-team setups.

## Test scenarios

- Fresh user: `cas cloud team set <uuid>` (no flag) → global file written, every Cassy-initialized project sees the team
- User opts into per-project: `cas cloud team set <uuid> --project` → only the current dir gets it; other projects fall back to global
- Existing per-project user (from before this change): per-project value still resolves correctly even after global is set to a different team
- `cas cloud team show` output identifies whether the active team came from project-level or global config
- `cas cloud team clear` with no flag clears the global; with `--project` clears the local override

## Verification fixtures

**Live box — `root@starscream`, Ben's account:**

- 19 existing `~/projects/*` to test bulk-apply behavior
- Current per-project config lives in `.cas/cloud.json` inside each repo (confirm exact path/schema from code — not audited in detail here)
- Ben's Cassy env at `~/.config/cas/env` provides token + endpoint

**Schema decision you need to make:**

- Global file path/name (`~/.cas/team.json` vs extending `~/.cas/cloud.json`) — look at existing `~/.cas/` artifacts (`cas.db`, `cloud.json`, `backup/`, factory sockets) and pick the cleanest fit. `cloud.json` already exists there — probably the right home.
- Resolution precedence: **per-project > global**. Do not reverse.

**Team UUID for verification:** `2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb` (Petra Stella, the only real team in the cloud right now).

**Reproduction smoke test:**

1. On starscream as ben: pick a project with no team set (one of Ben's 19 that he hasn't touched)
2. Today: `cd ~/projects/<unset-project> && cas cloud team-memories` → "No team configured"
3. After your fix: `cas cloud team set <uuid>` from `~` once, then the above `cas cloud team-memories` works in every unset project without further setup
4. Also test: a project where team was previously set to a DIFFERENT uuid via old per-project config — must still resolve to that uuid, not the new global default

## Acceptance criteria

Single `cas cloud team set <uuid>` (no flag) applied globally — verified by 5 different projects all reporting the team without any further config. `--project` flag still creates per-project override that takes precedence. Pre-existing per-project configs continue to resolve correctly. `cas cloud team show` indicates resolution source. All five test scenarios pass.

## Demo

From any directory: `cas cloud team set <uuid>`, then `cd ~/projects/ozer && cas cloud team show` and `cd ~/projects/cas-src && cas cloud team show` — both report the same team without any further setup. Then `cas cloud team set <other-uuid> --project` in cas-src reports a different team only there.

## Non-goals

- Does not change team-membership semantics in cloud DB
- Does not change `cas cloud team-memories` pull behavior (separate request — see `FEATURE-cloud-sync-pull-team-memories.md` / cas-e38e)
- Does not introduce per-org or per-host scoping — just project-overrides-global, two levels

## Coordination

Sibling: `FEATURE-cloud-sync-pull-team-memories.md` (cas-e38e). Shared code paths around team-config read — probably one worker takes both.

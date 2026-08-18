---
from: Petra Stella Cloud team
date: 2026-04-23
priority: P2
cas_task: cas-e38e
---

# `cas cloud sync` should also pull team-memories for active project

## Problem

When a user runs `cas cloud sync` from a Cassy-initialized project directory with a configured team, the command pulls only personal-scope data. Team-scoped memories for the project's canonical id are silently excluded — the user has to know about the separate `cas cloud team-memories` command.

The "Full sync (push then pull)" naming is misleading when team scope is silently dropped.

## Motivation

Discovered during Ben onboarding (2026-04-23). After 206 personal entries were promoted to team scope across 7 projects, Ben ran `cas cloud sync` from `~/projects/ozer` and it reported **0 entries synced**. He only got his 39 team memories after learning about `cas cloud team-memories` and running it manually.

## Files (cas-src)

- Modify: `src/cli/cloud.rs` (or wherever the `sync` subcommand handler lives)
- Reference: existing `cas cloud team-memories` implementation as the pull-team-scope pattern to invoke

## Approach

After the existing personal pull completes, if a team is configured for the current project AND the project has a canonical id, additionally invoke the team-memories pull for that project.

- No team or no canonical id → behave exactly as today (silent skip, no warning)
- Do NOT change push behavior
- Print personal and team counts as separate lines in the summary

## Test scenarios

- Happy path: project with team configured + canonical id → personal pull + team-memories pull both run, summary lists both counts separately
- No team configured → behaves identically to today
- Team configured but project has no canonical id → personal pull runs, team pull skipped silently
- Network error on team pull after successful personal pull → personal counts shown, team failure reported as a warning (not a hard error that loses personal pull info)

## Verification fixtures

**Database state (petra-stella-cloud Neon project `gentle-butterfly-19534503`):**

- Team "Petra Stella" UUID: `2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb`
- 206 entries promoted to team scope across 7 projects: cas-src (77), taxes (44), ozer (39), domdms (28), petra-stella-cloud (10), abundant-mines (7), closure-club (1)
- Daniel user_id: `3535edb0-a949-4200-883d-3c2c0d46de77`
- Ben user_id: `f7f4ca76-c7f0-427d-9e1f-e1e1b0430d7a`

**Live verification box — `root@starscream` (Hetzner), Ben's account:**

- 19 Cassy-initialized projects under `~/projects/` (ozer, domdms, cas-src, petra-stella-cloud, abundant-mines, closure-club, gabber-studio, …)
- Ben's Cassy env at `~/.config/cas/env` provides `CAS_CLOUD_TOKEN` and `CAS_CLOUD_ENDPOINT=https://petra-stella-cloud.vercel.app`
- `~/.zshrc` auto-logs in on shell start and defines `cas-login` alias

**Reproduction smoke test (from ben's account):**

1. `ssh root@starscream`, `su - ben`
2. `cd ~/projects/ozer`
3. `cas cloud sync` — today reports "0 entries synced" for team stuff; after your fix should report both personal AND team counts in one command.

## Cloud API already in place

- Team pull endpoint: `app/api/teams/[teamId]/sync/pull/route.ts` — the same endpoint `cas cloud team-memories` already hits. Your sync handler just needs to invoke the same pull after the personal pull.

## Acceptance criteria

From a project dir with team configured, a single `cas cloud sync` invocation pulls both personal and team-scoped entries. Output explicitly lists team-memory counts. Behavior unchanged when no team is set or project has no canonical id. All four test scenarios pass.

## Demo

User `cd`'s into `~/projects/ozer`, runs `cas cloud sync`, sees a single output containing both personal sync counts and `Team memories: 39 merged` without running any second command.

## Non-goals

- Does not change `cas cloud team-memories` behavior
- Does not auto-pull for projects other than the current one
- Does not change global team config behavior (see sibling request `FEATURE-global-team-config.md` / cas-eb38)

## Coordination

Sibling: `FEATURE-global-team-config.md` (cas-eb38). Both touch team-config read paths — probably one worker takes both.

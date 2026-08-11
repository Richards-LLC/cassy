# Slack draft — v12 burn-down landing (PRs #241/#242 → main), 2026-08-11

Channel: #cas-internal (C0B44GUKDK2)

## User thread

**Top-level:**
Live on production — **User** — Messages to idle Codex agents now reliably wake them, and every agent starts with a leaner, sharper memory: the always-loaded skill list shrank by two-thirds, so more of the context window belongs to your actual work.

**Reply (Was → Now):**
- Was: a message sent to an idle Codex agent could sit unread indefinitely — the agent looked alive but never acted, and stuck agents sometimes left ghost terminals behind after cleanup. Now: delivery is watched end-to-end, an unread message wakes the agent, and cleanup really tears everything down.
- Was: Codex agents launched into a trust prompt and stalled until someone clicked through, and fresh installs shipped without safety hooks armed. Now: every install carries the safety hooks pre-trusted, verified before launch — agents start working immediately, safely.
- Was: agent sessions began with a bulky boilerplate bundle (and team-launched sessions missed their project memory entirely). Now: every session — however it's launched — gets its project memory, and the boilerplate is 66% smaller.
- Was: a cloud cache outage could fail an entire test run for unrelated code. Now: the build notices, falls back to an uncached build with a loud note, and your change still gets validated.

## Dev thread

**Top-level:**
Live on production — **Dev** — PR #241 lands the delivery-liveness watchdog, locked Codex trust provisioning, canonical hooks.json serialization, and sccache fail-open across all CI lanes; PR #242 records the nextest-archive experiment as a measured negative result.

**Reply (Was → Now):**
- Was: NORMAL prompt injection to an idle Codex pane deferred forever on a composer-dirty gate; heartbeat reaps deregistered workers but abandoned live panes. Now: PTY submit classification + a delivered-unsurfaced watchdog guarantee turn observation, and stale maintenance routes through forced shutdown with direct-child wait (PRs #241; fixes #224, #236).
- Was: Codex spawn pre-trust used a timeout-and-proceed sidecar lock with no read-back; hooks.json regeneration wasn't byte-stable; generated hook commands drifted from the committed contract. Now: a blocking flock + fsync + read-back transaction gates spawn, one canonical JSON serializer emits hooks.json (zero-diff regen test), and provisioning writes harness-aware commands + trusted_hash entries idempotently across init/update/worker paths (fixes #235, #237).
- Was: mozilla-actions/sccache-action failures (backend 503s, cert errors) hard-failed jobs at setup or post. Now: continue-on-error + probe with a no-op stats fallback on all five setup sites, pinned by 100+ tier-contract assertions (fixes #234).
- Was: session-start ambient context skipped team-spawned sessions; retrieval ignored outcome stats; 29.8KB of skill descriptions injected everywhere. Now: SessionStart hook wired for factory roles (fixes #239), retrieval outcomes feed recall scoring, descriptions cut to 10.2KB with pin tests green.
- Was: hoped exact-SHA nextest archives would cut the warm required path under 5m. Now: measured — 8m33 fresh-SHA vs 6m06 baseline; machinery not landed, floor analysis + next levers in docs/analysis (PR #242, PR #240 closed unmerged).

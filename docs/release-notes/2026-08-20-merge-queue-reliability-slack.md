# 2026-08-20 — Merge-queue reliability — Slack draft

Channel: `#cas-internal` (`C0B44GUKDK2`)
Covers main merges: PR #538, #539, #540, #541.

## User thread

**Top-level**

> **Live on production — User**
> Code changes now sail through the automatic merge queue and land on main in about ten minutes — no more merges stuck for hours or dying on infrastructure hiccups.

**Reply**

> **Was:** merging a change could stall for hours — the fast build machine kept rejecting its work at the last step, and stuck jobs quietly hogged the pipeline until someone noticed and cleared them by hand. Two release publications once sat unnoticed for almost two weeks.
> **Now:** the queue validates and merges changes end-to-end on its own — the fast build machine's results are accepted every time, and two watchdogs automatically clear anything stuck or outdated within minutes instead of waiting for a human.

## Dev thread

**Top-level**

> **Live on production — Dev**
> The self-hosted merge-queue lane is green end-to-end: archive builds on the private runner now execute cleanly on hosted shard runners, and stale/superseded CI runs cancel themselves.

**Reply**

> **Was:** merge_group runs failed on a chain of cross-machine defects — a poisoned compiler-probe cache killed `cargo metadata`, the shared `CARGO_TARGET_DIR` broke CLI packaging, and producer paths baked into test binaries (nextest workspace, insta snapshot roots, `CARGO_BIN_EXE_*`/`CARGO_MANIFEST_DIR`) broke every hosted shard. Orphaned queue runs and superseded heavy-tier validations squatted on capacity for hours to days with no auto-cancel.
> **Now:** the archive lane is fully portable — compiler wrapper cleared for queue builds, target-dir-aware packaging, `--workspace-remap`, `INSTA_WORKSPACE_ROOT`, and gated producer-path shims (#538); a flaky PTY test race is fixed (#539); the runner group policy, committed systemd provisioning (`SCCACHE_IDLE_TIMEOUT=0`, `CARGO_CACHE_RUSTC_INFO=0`), and a 5-minute merge-queue watchdog are durable (#540); heavy-tier lanes carry per-ref concurrency cancellation and a second watchdog reclaims non-queue runs stuck in queued (#541). Receipts: queue claim 8s, archive build 88s, PR open→merged 11m50s.

## POSTED

- UTC: 2026-08-20T12:27Z (all four messages)
- Channel: `#cas-internal` (`C0B44GUKDK2`)
- User top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787228842949429 (`ts 1787228842.949429`)
- User reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787228847501989
- Dev top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787228851364689 (`ts 1787228851.364689`)
- Dev reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787228858940029

# 2026-08-20 — Sub-5-minute merges — Slack draft

Channel: `#cas-internal` (`C0B44GUKDK2`)
Covers main merges: PR #542, #543, #544, #545, #547, #548, #550.

## User thread

**Top-level**

> **Live on production — User**
> A code change now goes from "submitted" to "live on main" in under five minutes — measured today at 4 minutes 58 seconds, down from over twenty.

**Reply**

> **Was:** getting a change onto main took over twenty minutes on a good day — every submission re-ran heavy duplicate checks before it could even enter the merge line, and a stuck helper could silently stall the whole team.
> **Now:** submission checks pass in seconds, the merge line does the full validation once on fast machines, and a real change measured 4m58 from opened to merged. Stopped or stuck helpers now always raise their hand automatically instead of going quiet.

## Dev thread

**Top-level**

> **Live on production — Dev**
> Open→merged for a rust-touched PR measured 4m58 (queue-created→merged 4m27): PR admissions collapsed to 3–4s, archive builds run 51s on the self-hosted box, and archived test binaries are now fully runtime-path-portable with the consumer shim deleted.

**Reply**

> **Was:** PR admission cost a duplicate ~6m macOS compile before queue entry (#544 baseline: 5m58 queue-only, 58s over target); archived test binaries baked producer paths (`CARGO_BIN_EXE_*`, `CARGO_MANIFEST_DIR`, insta roots, `assert_cmd::cargo_bin`) that broke hosted shards, papered over by consumer symlinks; worker stoppage classes (silent delivery stalls, usage-limited harnesses with live heartbeats) never reached a durable supervisor alert; watchdog scripts were text-asserted only and defaulted to production; sccache died with EAGAIN under parallel load (cgroup TasksMax=512).
> **Now:** required macOS/Fast Validation on PR heads are short admission receipts with full coverage retained on every merge-group tree (#544); every archive consumer resolves paths at runtime via a workspace-walking resolver and the CI shim is deleted, proven by a producer-target-renamed cross-directory run plus a green queue run (#550); worker stoppage relays are durable, ACK-bridged, persistence-confirmed, and episode-stable (#542, #547); both watchdogs share one threshold authority with a fake-gh behavioral suite, explicit-repo guard, dry-run, and job-scoped measurement (#545); tier-contract assertions are step-scoped with mutation coverage (#543); the runner unit reserves 2,048 cgroup task slots, fixing the measured sccache spawn failure (#548).

## POSTED

- UTC: 2026-08-20T14:18Z (all four messages)
- Channel: `#cas-internal` (`C0B44GUKDK2`)
- User top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787235491398239 (`ts 1787235491.398239`)
- User reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787235497538219
- Dev top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787235501956039 (`ts 1787235501.956039`)
- Dev reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787235510085589

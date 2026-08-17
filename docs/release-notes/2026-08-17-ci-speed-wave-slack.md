# 2026-08-17 — CI speed wave — Slack draft

Channel: `#cas-internal` (`C0B44GUKDK2`)
Deploy target: **Live on production** (merged to `main`; PRs #467, #468, #474, #475, #476)

## User thread

**Top-level:**

> **Live on production — User:** Getting a change into main and shipping a release both got dramatically faster — documentation and web-UI changes now clear their required checks in about four minutes instead of ten, and each merged change no longer triggers twenty minutes of redundant re-testing.

**Reply:**

> **Was →** Every pull request paid the full ten-minute Rust test suite even when it only touched docs or the web Commander, and every merge to main re-ran another twenty-plus minutes of heavy validation on code that had just been validated. **Now →** Required checks recognize what a change actually touches — out-of-scope changes clear in minutes — the Rust suite runs as parallel shards from a single build, and post-merge validation skips work it can prove already ran on the identical code, loudly citing the earlier run.
>
> **Was →** Publishing a release took about 26 minutes from tag to downloadable builds. **Now →** The platform builds start immediately and run concurrently with verification, with a projected 12–15 minute path — the next release measures it for real. Every integrity audit on the shipped binaries is unchanged.
>
> **Was →** Two timing-sensitive tests failed randomly on busy CI machines, repeatedly blocking merges that had nothing wrong with them. **Now →** Both are contention-proof; a busy machine can no longer fail a healthy change.

## Dev thread

**Top-level:**

> **Live on production — Dev:** The CI graph was rebuilt around three ideas — classify the diff before running required jobs, build test binaries once and fan them out, and never revalidate a tree hash that already passed (PRs #467, #468, #474–#476).

**Reply:**

> **Was →** `Fast Validation — full suite` compiled and ran everything on every PR (7–11 min critical path), and main pushes re-ran Panic Isolation ×2 plus the cold Build Benchmark (22/12/15 min) on PR-validated trees. **Now →** Required jobs first classify the merge-base diff (docs-only / hub-web-only / rust-touched, fail-closed on uncertainty) and short-circuit with an explicit skip rationale; the rust path builds one nextest archive and fans it to three test-only partitions (tar-packaged so executable bits survive artifact transit); main-push heavy jobs check a per-tree receipt and skip with the prior run URL when the identical tree already passed. Panic profiles and the benchmark moved to schedule/manual. Tier policy stays pinned by the contract script (185 checks). (#467, #468)
>
> **Was →** Release jobs ran serially on the full fat-LTO profile. **Now →** Verification and both platform builds start concurrently and build with thin-LTO (`release-fast`) plus explicit strip — measured tradeoff +4.25 MB compressed / +0.43 ms startup — with BLAKE3 and portable-ISA audits unchanged; projected 12–15 min tag-to-assets, to be measured on the next real tag. (#476)
>
> **Was →** Two tests asserted real properties with contention-sized margins: the PTY Esc test's 20s event ceiling and the proxy startup-concurrency test's 55ms scheduler slack — three mainline/PR reds today between them. **Now →** Both use wide-margin constants (120s event ceiling; 400ms/700ms/≥800ms concurrent-vs-serial windows) with the failing run IDs cited in comments. (#474, #467)
>
> The full evidence base — per-lane timings, sccache hit rates (0–6% pre-cache-v2 → 92% warm), and the ranked remaining measures (persistent test-binary cache for the archive producer, merge queue, self-hosted runner pilot) — is in `docs/analysis/2026-08-17-ci-speed-spike.md`. (#475)

## POSTED

- **When (UTC):** 2026-08-17 ~23:06
- **Channel:** `#cas-internal` (`C0B44GUKDK2`)
- **User top-level:** https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787007937326999 (`ts 1787007937.326999`)
- **User reply:** https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787007945187629
- **Dev top-level:** https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787007950128989 (`ts 1787007950.128989`)
- **Dev reply:** https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787007961675699

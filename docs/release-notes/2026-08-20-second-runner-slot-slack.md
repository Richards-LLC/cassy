# 2026-08-20 — Second trusted runner slot for the merge queue — Slack draft

Channel: `#cas-internal` (`C0B44GUKDK2`)
Covers main merges: PR #580, #581, #582.

## User thread

**Top-level**

> **Live on production — User**
> The merge queue's Linux checks no longer wait in a single-file line: a second build slot runs them side by side, and the honest numbers say the remaining wait is the Mac check, not Linux.

**Reply**

> **Was → Now**
> - *Was:* the private Linux build box had one listener, so the three compile-heavy merge-queue jobs ran one after another (about six minutes of pure queueing on the box).
>   *Now:* two independent listeners on the same box, each with its own build cache, so two of those jobs start together and the third waits for at most one. In the second measured run every Linux build job was finished 3 minutes 15 seconds after the run started.
> - *Was:* the plan said "under six minutes end to end" without evidence either way.
>   *Now:* measured and written down: whole-queue times were 7m54 and 7m39 for the two runs, and in both the last thing to finish was the hosted Mac check (7m37 / 7m22). The six-minute target is not met yet, and the docs say so plainly — the next lever is the Mac lane, not more Linux capacity.

## Dev thread

**Top-level**

> **Live on production — Dev**
> `soundwave-cas-ci-2` is registered as a second org-level listener with its own Cargo target, sccache dir and port; every trusted self-hosted lane now fail-closes on an exact slot tuple, and two consecutive queue receipts (7m54, 7m39) pin hosted macOS as the critical path.

**Reply**

> **Was → Now**
> - *Was:* one `cassy-actions-runner.service` listener; the merge-queue trust steps checked `CARGO_TARGET_DIR`/`SCCACHE_DIR` prefixes and `SCCACHE_SERVER_PORT=4227` independently, so a mixed configuration could have shared a Cargo lock or cache server unnoticed. (PR #580)
>   *Now:* `scripts/install-cassy-actions-runner.sh` takes `RUNNER_SLOT=1|2` (explicit per-slot name, checkout, target, sccache dir, unit and wrapper; anything else rejected); `ops/systemd/cassy-actions-runner-2.service` mirrors slot 1's hardening (non-root, `NoNewPrivileges`, `ProtectSystem=strict`, empty capability set, `nice 10`, `CARGO_BUILD_JOBS=12`, `TasksMax=2048`) on `cargo-target-2` / `sccache-2` / port 4228. New `scripts/check-cassy-actions-runner-isolation.sh` accepts only the two exact target/cache/port tuples plus non-root; the merge-queue suite-build/preflight/doctests trust steps, the self-hosted pilot workflow and `check-release-runner-trust.sh` all delegate to it (fail-closed), while the fail-open sccache probe no longer owns the decision. `scripts/test-ci-test-tiers.sh` (632) and `test-check-release-runner-trust.sh` (13, incl. mixed-tuple rejection) pin it.
> - *Was:* no receipts for a two-listener queue. (PR #581, #582)
>   *Now:* `docs/ci/self-hosted-runner-pilot.md` records both consecutive runs with per-job runner names: #580 — archive 1m38 (slot 1) ∥ preflight 3m36 (slot 2, cold), doctests 1m21 (slot 1), Linux Fast Validation 4m02, macOS 7m37, total 7m54; #581 — archive 3m00 (slot 2) ∥ doctests 1m22 (slot 1), preflight 1m31 (slot 1), all self-hosted jobs done at +3m15, hosted shards stretched the Linux rollup to 6m12, macOS 7m22, total 7m39. Option B (hosted cache-warm on main push) was evaluated against the #568 numbers and rejected as cache-dependent and blind to macOS variance. Target not met; floor and critical path recorded honestly.

## POSTED

Posted 2026-08-20 ~22:02 UTC to `#cas-internal` (`C0B44GUKDK2`):

| Message | Permalink |
|---|---|
| User top-level | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787263330320019 |
| User reply | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787263336213909?thread_ts=1787263330.320019&cid=C0B44GUKDK2 |
| Dev top-level | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787263337241789 |
| Dev reply | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787263347561769?thread_ts=1787263337.241789&cid=C0B44GUKDK2 |

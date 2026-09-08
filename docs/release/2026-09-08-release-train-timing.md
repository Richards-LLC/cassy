# Release-train timing, 8 September 2026

The largest proven costs are a cold publication audit, repeated full-suite
execution, and supervisor hand-off time. The saved evidence does **not** support
"every gate takes 25 minutes": the second full 3.18.1 gate took 319 seconds.
A further 952 seconds elapsed before its pipeline was launched.

## Evidence and limits

Sources are the saved files under `~/.cas/artifacts/release/` in
`v3.18.1-release-3181-merge` and `v3.19.0-epic-92c0-merge`, plus the timestamped
CAS notes on `cas-4c4b` and `cas-92c0`. Times below are UTC on 2026-09-08.
The historical gate printed only verdicts, discarded successful row output,
and replaced `gate.log` on a full retry. Neither per-row duration nor per-row
CPU usage survives. "Unavailable" is deliberately not a zero or an estimate.
The supervisor accepted this evidence limit and the correction to gate 2's
claimed duration on 8 September.

### Gate rows

Both historical trains used sequential dispatch. Each cell gives the surviving
verdict for the second attempt; wall/user/system seconds are unavailable for
**every** historical row. The first attempt's row logs were overwritten.

| Row | 3.18.1 attempt 2 | 3.19.0 attempt 2 | Wall / user / system seconds |
| --- | --- | --- | --- |
| scratch-base | PASS | PASS | Unavailable |
| epic-worktree-fresh | PASS | PASS | Unavailable |
| epic-worktree-zig | PASS | PASS | Unavailable |
| failure-log | PASS | PASS | Unavailable |
| ancestor-proxy-config | PASS | PASS | Unavailable |
| version-literals | PASS | PASS | Unavailable |
| fixture-paths | PASS | PASS | Unavailable |
| workspace-tests | PASS | PASS | Unavailable |
| hub-web-dist-drift | Absent | PASS | Unavailable |
| hub-web-visual-qa | PASS | PASS | Unavailable |
| nextest | PASS | FAIL | Unavailable |
| doctests | PASS | PASS | Unavailable |
| archive-mode | PASS | FAIL | Unavailable |
| snapshot-portability | PASS | PASS | Unavailable |
| builtin-projections | PASS | PASS | Unavailable |
| changelog-and-versions | PASS | PASS | Unavailable |
| release-script | PASS | PASS | Unavailable |
| procedure-guardrails | PASS | PASS | Unavailable |
| working-tree | PASS | PASS | Unavailable |

### Phases and hand-offs

| Train / phase | Start | End | Seconds | Evidence quality |
| --- | --- | --- | ---: | --- |
| 3.18.1 gate 1 | 12:51:29 | by 13:16 | ≤1,471 | Start and completion bound from note; receipt overwritten |
| 3.18.1 first pipeline | about 13:17 | failed by 13:25 | ≤480, approximate | Note: queue rejected stale committed web dist |
| 3.18.1 gate 2 | 13:25:44 | 13:31:03 | 319 | Note launch; root `gate.green.epoch=1788874263` and done mtime agree |
| 3.18.1 hand-off, gate green to pipeline launch | 13:31:03 | 13:46:55 | 952 | Epoch receipt and `pipeline-run2.log` |
| 3.18.1 pipeline launch to PR identified | 13:46:55 | 13:46:56 | 1 | Existing PR #747 reused |
| 3.18.1 PR identified to queue admitted | 13:46:56 | 13:46:59 | 3 | Pipeline log |
| 3.18.1 queue admitted to merged detection | 13:46:59 | 13:55:30 | 511 | Queue run 34234134542; detected merge, not exact GitHub merge timestamp |
| 3.18.1 hand-off, merged detection to publisher | 13:55:30 | 13:58:27 | 177 | Pipeline and publish logs |
| 3.18.1 local publisher audit/tag launch | 13:58:27 | 14:11:51 | 804 | `publish.log` |
| ↳ development preflight build | — | — | 98 | `release.log`: `Finished dev ... in 1m 38s` |
| ↳ release-profile build | — | — | 674 | `release.log`: `Finished release ... in 11m 14s` |
| 3.18.1 tag push to assets published | 14:11:53 | 14:14:47 | 174 | Verified latency receipt; workflow 34236714630 |
| 3.18.1 green to published | 13:31:03 | 14:14:47 | 2,624 | Final full-gate epoch to published receipt |
| 3.18.1 initial gate to published | 12:51:29 | 14:14:47 | 4,998 | 83m18s, including failed attempt and hand-offs |
| 3.19.0 gate 1 | 17:34:34 | by 17:58 | ≤1,406 | Note bounds; original receipt overwritten |
| 3.19.0 fix / hand-off before gate 2 | after gate 1 | about 18:15 | Unavailable | Includes reproduction, worker correction and merge; not a measured gate row |
| 3.19.0 gate 2 | about 18:15 | 18:21:03 | about 363 | Launch minute from saved run files/task description; done mtime |
| 3.19.0 pipeline / queue / publish | — | — | Not reached | Both initial full attempts failed; no publication receipt existed at collection |

The 3.18.1 root `gate.full.sha` names `3afcccce`; there is no diagnostics
subdirectory or second per-attempt `gate.done` in the saved run directory.
Thus the 13:31:03 epoch is the full gate, not an `--only` diagnostic. The earlier
note's approximately 21-minute duration included hand-off time. The supervisor
identified reminder cadence and manual dist regeneration as contributors.
The 3.19.0 attempt-2 failure was an inherited factory-session environment leak,
not a regression in the prior close-target change (corrected epic note 18:34).

## Prioritized plan

1. **Remove hand-off latency where receipts already authorize the next step.**
   The proven opportunity is 952s before pipeline and 177s before publisher.
   An explicit, opt-in supervised gate→pipeline continuation should require a
   prepared PR body and invoke the existing exact-SHA pipeline check after the
   full gate succeeds. Give it a recorded PID/process group and independent
   pipeline terminal receipt. This removes reminder cadence without inferring
   permission to publish. It is a follow-up, not an authorization relaxation.
2. **Move publication audit off the green-to-published critical path.**
   The observed local audit costs 804s (674s release compilation). Start an
   audit before queue completion, then reuse only when source tree, toolchain,
   target flags, embedded revision and secret-dependent build inputs match the
   landed commit. `release.sh` currently runs `cargo clean --release --target`
   for Linux, so merely running its bare audit mode early does **not** prewarm
   the subsequent publish: it cleans again. A verified artifact hand-off is
   necessary. Potential saving: up to the overlapping portion of 804s; no
   measured saving is claimed here. Retain ISA, preflight and publication checks.
3. **Execute workspace tests once in the archive environment (implemented).**
   The old gate executes `nextest run --workspace`, then builds an archive and
   executes almost the same suite again. Cargo may already reuse compilation;
   the source proves duplicate execution, not two independent cold compiles.
   The in-tree nextest row now runs only `component_output_test` when archive
   mode is selected; the archive row runs its exact complement across the whole
   workspace. A focused `--only nextest` keeps full in-tree execution.
   Expected saving: one workspace execution minus the snapshot complement,
   plus associated harness startup. Historical seconds cannot be recovered.
4. **Reuse eligible unchanged PASS rows on a full retry (implemented).**
   `--gate --reuse` still evaluates the complete row set and records the new
   exact SHA. Rust checks depend conservatively on the entire commit; both web
   rows depend on tracked `hub-web`, `scripts`, and `.github` inputs. A Rust-only
   fix can reuse web proofs while rerunning Rust. An unchanged full retry can
   reuse all eight eligible expensive rows. Live scratch/identity checks,
   version/procedure checks, ledger regeneration and final cleanliness run each
   time. Expected saving is the sum of the reused rows' retained durations;
   no fixed 25-minute promise is supported by the historical receipts.
5. **Budget concurrency before parallelizing Cargo rows.**
   Independent web work can overlap suite execution, but shared Cargo target
   locks serialize competing Cargo writers and increase resource contention.
   Prefer one archive producer plus read-only consumers with an explicit CPU
   allocation. Do not add concurrent cold builds to an already busy host.
6. **Reduce queue polling only after the larger costs.**
   Saved polls are about 46 seconds apart. Queue success was visible at
   13:54:43; merged was detected at 13:55:30. That bounds detection overhead,
   not queue execution. A shorter interval may save tens of seconds, while
   increasing API traffic. Keep bounded retries and cancellation behavior.

## Receipt and environment contract

Each train attempt now retains `rows/<UTC>-<pid>/<row>.log`, visual-QA evidence,
and `timing.tsv`: row, UTC start/end, wall seconds, user CPU seconds, system CPU
seconds, status, source SHA. Bash `time` measures the function and its children
without losing the Zig resolver's exported environment. CPU seconds can exceed
wall seconds because Cargo runs children concurrently. Cache hits use `REUSED`
and zero execution time; lookup overhead is not represented as test execution.
`gate.log` also prints each executed row's UTC interval and timings.

The full-gate cache records only clean-tree PASS results, keyed by explicit
inputs, gate implementation bytes, version, checkout identity and hashed
caller environment/tool versions. Environment values and credentials are never
written into the key or receipt. Receipts expire after 24 hours; missing,
malformed, future or expired records rerun the row. Unknown tool identity
turns reuse off. `--only` neither reads nor populates this cache. Default full
gates populate it but rerun every row unless `--reuse` was explicitly requested.
`--pipeline` continues to require successful `gate.done`, `gate.full.sha`, a
clean tree and the same current commit; a cached individual row cannot start it.

Queue parity on base `456941cc`: the `hub-web-dist-drift` row from cas-83ff runs
`npm ci`, `npm run build`, and `git diff --exit-code -- dist`. Both nextest and
archive paths retain cas-6df6's scrub of the five factory identity variables.
The archive consumer keeps missing sccache, empty Cargo home, unset CAS_ROOT /
COLUMNS and workspace remapping. Both suite execution paths now use CI's verified nonzero-test wrapper and
`--no-fail-fast`; the archive consumer pins `INSTA_WORKSPACE_ROOT` to its remap.
CI's three partitions remain a scheduling difference: running their complete
union in one invocation preserves coverage while avoiding competing local
resource demands.

## Validation

Fixture self-tests exercise suite coverage, cache invalidation, expiry, dirty
working trees, environment changes, and diagnostic isolation. Train self-tests
exercise forwarding and exact-SHA receipts, including rejection of diagnostic
receipts at the pipeline boundary. Production before/after timings are pending;
fixture durations must not be presented as production speedups.

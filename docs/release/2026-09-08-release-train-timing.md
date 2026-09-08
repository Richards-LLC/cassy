# Release-train timing, 8 September 2026

The largest proven costs are a cold publication audit, repeated full-suite
execution, and supervisor hand-off time. The saved evidence does **not** support
"every gate takes 25 minutes": the second full 3.18.1 gate took 319 seconds.
A further 952 seconds elapsed before its pipeline was launched. The final
3.19.0 gate took 317 seconds, followed by a 600-second hand-off. Its queue
phase took 970 seconds and its local publisher phase took 1,161 seconds.

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
| 3.19.0 final gate | 19:51:36 | 19:56:53 | 317 | Final run.env and gate.green.epoch; intermediate failed attempts are not reconstructed |
| 3.19.0 hand-off, green to pipeline | 19:56:53 | 20:06:53 | 600 | Epoch receipt and pipeline.log |
| 3.19.0 pipeline launch to PR identified | 20:06:53 | 20:06:56 | 3 | PR #761 |
| 3.19.0 PR identified to queue admitted | 20:06:56 | 20:07:46 | 50 | Includes required-check wait |
| 3.19.0 queue admitted to merged detection | 20:07:46 | 20:23:56 | 970 | Queue run 34273007205 |
| 3.19.0 hand-off, merged to publisher | 20:23:56 | 20:24:08 | 12 | Pipeline and publish logs |
| 3.19.0 local publisher audit/tag launch | 20:24:08 | 20:43:29 | 1,161 | publish.log |
| ↳ development preflight build | — | — | 53.11 | release.log Cargo summary |
| ↳ release-profile build | — | — | 423 | release.log Cargo summary; remainder of publisher phase lacks substep timing |
| 3.19.0 tag push to assets published | 20:43:31 | 20:46:23 | 172 | Verified latency receipt; workflow 34276474931 |
| 3.19.0 green to published | 19:56:53 | 20:46:23 | 2970 | Final gate epoch to verified publication |

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
   The observed local audits cost 804s in 3.18.1 (674s release compilation)
   and 1,161s in 3.19.0 (423s release compilation). Start an
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
   Reserve an exclusive host window for release measurements: no live release
   gate, CI suite, factory build or publish audit may overlap. Limiting Cargo
   build jobs alone does not bound nextest's runtime concurrency. The failed
   measurement below demonstrates why CPU budgeting is insufficient without
   host scheduling.
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
The doctest row also uses CI's verified nonzero-test wrapper and scrubs all five factory identity variables. CI's three partitions remain a scheduling difference: running their complete
union in one invocation preserves coverage while avoiding competing local
resource demands.

## Validation and measurement status

At checkpoint `a99ee8ef12ae93ae57c4684a92cc44b2c0894509`, the gate self-test
passed 71 cases and the train self-test passed 124, both with exit status zero.
They cover complementary suite filters, zero-test rejection, cache invalidation,
expiry, dirty trees, environment changes, web diagnostic forwarding, exact-SHA
receipts and refusal of diagnostic receipts at the pipeline boundary. Terminal
QA passed 11 captures; its 17 allowed findings concern existing path/command
width and Unicode separators, not the new timing lines. Full production flavor
drift validation and an uncontended before/after comparison remain pending.

### Discarded concurrent-host attempt

The attempted real measurement began at 19:05:07Z on the checkpoint above. Its
recorded process group was 3010712 and Cargo build concurrency was limited to
four. The driver intended to warm once, then compare an instrumented full
in-tree baseline with complementary execution and an unchanged `--reuse` run
on the same immutable source tree. Only the warmup ran before cancellation.

The supervisor ordered it stopped at 19:09 after identifying overlap with the
live 3.19.0 release gate run 3 on the same host. That release gate reported a
load-sensitive spawn-test failure. The shared load confounds both the release
check and the benchmark. SIGTERM was sent to process group 3010712; a subsequent
process-group query returned no members. There is no completed comparison and
**no valid production speedup measurement** from this attempt.

The partial raw logs and per-row CPU/wall values are retained for diagnosis
under `~/.cas/artifacts/cas-d136/measurement/`. Both `validity.json` and
`INVALID-DUE-TO-CONTENTION.md` mark the attempt `invalid-due-to-contention`.
These values must not be used to estimate normal row costs or savings. Its
row-cache directory is also part of the invalid trial and must not seed a new
performance experiment. The supervisor authorized a fresh trial at 20:57Z after confirming publication.
Each trial must find zero host cargo/rustc processes and load1 below four,
waiting at most ten minutes in thirty-second intervals; otherwise it is
recorded as skipped for contention.

### Remaining work and release follow-ups

After release sequencing permits an exclusive measurement window, start a new
evidence directory at the then-current candidate SHA, warm the private build
artifacts, and compare baseline, optimized and reused full gates with identical
resource limits. Preserve every row's raw output and nonzero-test counts. A
failure is a failed proof even if its duration is shorter. Update this report
with actual timings only after those conditions hold; do not substitute the
fixture tests' elapsed time for production measurements.

The historical receipts already justify investigating hand-off automation and
publication audit reuse. The next changes should remain separately reviewable:

- An explicit gate-to-pipeline continuation can remove the observed 952-second
  hand-off. It must require a prepared PR body, run the existing exact-SHA check,
  retain a recorded process group, stop on any failed row and write a separate
  pipeline terminal receipt. Publication remains a separate explicit action.
- A prewarmed local publisher needs a validated artifact receipt, not just a
  background invocation of the current audit-only command. That receipt must
  cover landed revision, toolchain, target/ISA flags, embedded build metadata
  and secret-dependent build inputs without exposing their values. On mismatch,
  rebuild and rerun the existing audits. Avoid the unconditional Linux clean
  only when this evidence proves reuse safe.
- Shorter bounded queue polling can reduce tens of seconds of detection delay;
  it cannot remove actual runner queue time or justify bypassing queue checks.

The implementation checkpoint delivers duplicate-suite removal, conservative
row reuse and durable measurements. It does not yet establish the requested
end-to-end latency reduction or deliver those larger follow-up optimizations.

### Follow-up implementation specifications

The supervisor authorized the implemented pair and parity fixes first, with
these larger changes delivered as specifications if context does not permit
safe implementation in this task. Both specifications preserve publication as
an explicitly requested action and must ship their own script self-tests.

**Continuation:** add an explicit full-train mode to `release-train.sh` that
accepts a prepared PR body and an explicit publication selection before launch.
Its recorded process group owns gate → pipeline → optional publisher; `--stop`
terminates all descendants. After gate success, call the existing pipeline
boundary so exact SHA, clean tree and full receipt are checked again. Require
`pipeline.done=MERGED`, the recorded landed SHA and matching origin/main before
calling the existing publisher. Keep separate gate, pipeline and publisher
terminal receipts, and distinguish tag completion from verified asset
publication. Resume checks recorded state and live PIDs before retrying a
phase; never infer success from a stale or partial file. Test dirty/stale SHA,
`--only`, failed gate, dropped queue, failed publish, cancellation, duplicate
launch and restart after each boundary. Update all three skill mirrors, add
`--learn`, and regenerate the ledger last. Proven upper bounds on removable
hand-offs: 1,129s in 3.18.1 and 612s in 3.19.0. Actual saving requires a fresh
train measurement; polling and network calls remain.

**Publisher artifact reuse:** first measure all substeps inside `release.sh`,
because the 1,161s 3.19.0 publisher phase includes only 476.11s identified Cargo
build time. Add an audit-only producer and a validated consumer receipt keyed
by exact source SHA/tree, toolchain versions, target and ISA flags, Zig, build
metadata (including embedded revision/date), build-script inputs and hashed
secret-dependent environment. Include artifact digests, successful preflight
and ISA audits; write the receipt atomically after completion. A missing,
expired, modified or mismatched receipt reruns the current full audit, including
Linux cleanup. No receipt authorizes tag push by itself. A pipeline-era build
cannot be reused for a different landed revision just because Git trees match:
embedded revision is observable. Establish the exact landed SHA before claiming
reuse or explicitly design and test the metadata rebuild. Test every input
mismatch, damaged/missing artifact, failed audit, concurrent producer and absence
of remote mutation in producer mode. Potential overlap is bounded by 804s /
1,161s of observed local publisher time; it is not a promised full saving, and
the unchanged workflow still needs 174s / 172s to publish assets.

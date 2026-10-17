# One broad test run belongs at final integration

<figure class="hero-chart" aria-describedby="hero-caption">
<svg viewBox="0 0 340 180" role="img" aria-labelledby="chart-title chart-desc">
<title id="chart-title">Delivery CI: measured 16.9 minutes to modeled 2.5 minutes</title>
<desc id="chart-desc">A descending slope joins a solid measured endpoint at 16.9 minutes to an outlined forecast endpoint at 2.5 minutes. The modeled reduction is 14.4 minutes, 85.1 percent, compared with the measured median.</desc>
<path class="axis" d="M40 26V148H302"/>
<path class="slope" d="M65 42L270 133"/>
<circle class="actual" cx="65" cy="42" r="7"/><circle class="forecast" cx="270" cy="133" r="7"/>
<text x="65" y="22" text-anchor="middle">16.9 min</text><text x="270" y="112" text-anchor="middle">2.5 min</text>
<text class="caption" x="65" y="170" text-anchor="middle">Measured</text><text class="caption" x="270" y="170" text-anchor="middle">Modeled</text>
<text class="delta" x="171" y="66" text-anchor="middle">−14.4 min</text>
</svg>
<figcaption id="hero-caption">30 merged v46 deliveries, October 16–17, 2026. Forecast removes the broad Rust and duplicate snapshot steps, retaining measured runner delays. Source: deliveries.csv and the paired replay in the evidence bundle, extracted October 17.</figcaption>
</figure>

Recommend reusing focused worker proof, removing broad Rust execution from both factory CI routes, and retaining the full integration suite in the merge queue. **Decision pending; no CI, hook, close gate, or runtime change is delivered here.**

The modeled lane improvement does not establish the release goal. Conservative replay predicts **20.4 minutes commit→epic merge**, versus 35.1 measured. An immediate, successful queue would bring that to **31.2 minutes commit→main**, still above the 30-minute goal. Actual batched delivery→main was much longer.

## Decision brief for the operator

Approve the design below before implementation under rule-176. The decisive choice is where broad coverage belongs: the final merged tree. Keep named regression tests, review, QA, merge evidence and refusal of pending/red code CI. Remove the extra whole-workspace branch run and the requirement to run arbitrary tests merely to obtain a receipt in every dependent package.

| Option | Cost and expected outcome | Risk and effort | Reversibility |
| --- | --- | --- | --- |
| Keep current flow | Measured median lane 16.9 min; commit→epic 35.1 min | Repeated broad tests and admission waits; no implementation | No action |
| **Recommended: focused worker proof + cheap factory CI + final-tree suite** | Modeled median lane 2.5 min; commit→epic 20.4 min; immediate commit→main 31.2 min | Moderate changes to existing routing and receipt consumption; integration-only defects surface in queue | Restore current routing/gate predicates |
| Narrow the impact selector but keep hosted Rust builds on every push | No defensible timing forecast from this sample; example compilation alone takes 4.2–7.2 min | Lower change in ownership; graph, opaque imports and rebases still widen selection | Restore selector policy |

What we give up: a second broad test run before the epic merge. The runner-up preserves that additional branch signal, but it repeats final integration coverage and cannot support a five-minute lane claim. Focused worker tests still fail early; the queue remains the gate to main. Apply the new policy only after source/receipt coverage is verified on the delivered SHA; missing focused proof must refuse admission or require the explicit targeted fallback, never silently claim full validation.

Reversal cost is one policy revert in existing tools/workflow; keep the old predicates available in Git. There is no new daemon, queue, assembly stage or proof service in this design.

## Measured results

All values below are minutes except sample counts. Figures round to one decimal; raw CSV/JSON retains seconds. Different-stage medians do not add to an end-to-end median.

| Metric | Measured | Modeled | Change versus measured | Sample |
| --- | ---: | ---: | ---: | ---: |
| CI run creation→last executed job completion | 16.9 | 2.5 | −14.4 (−85.1%) | 30 paired deliveries |
| Final delivery commit→first containing epic commit | 35.1 | 20.4 | −14.7 (−41.8%) | 30 paired deliveries |
| Final delivery commit→main | 268.7 | Not forecast under batching | Not established | 30 deliveries in one release |
| Successful merge-queue attempt | 10.5 | Unchanged | 0.0 | 1 successful attempt |
| Immediate commit→main, excluding release batching | Not observed as this population | 31.2 | Not a measured improvement | Replay plus one successful queue |

The median paired lane saving is 13.9 minutes; the difference between the two lane medians is 14.4. Both are valid statistics with different definitions. Lane interquartile observations are 15.0–18.2 minutes; the maximum is 93.2. In run [38962534324](https://github.com/Richards-LLC/cassy/actions/runs/38962534324), classification ended at 20:31:00Z but the fast job started only at 21:46:19Z. Keep that scheduling delay in the replay; this is not all test time.

Median commit→CI creation was 7.6 minutes; median CI-green→epic merge was 2.5 minutes. These include worker proof, handoff and operational waits, not measured CPU stages. The paired model retains both rather than assuming all coordination becomes instantaneous.

### What the two Scoped Validation rows actually do

28 of 30 runs executed Rust. Median selected count was **10,274.5 tests**; the impact step took **13.8 minutes**, including compilation and execution. Rust ran in the fast row on 13 deliveries and in factory/PR on 15; two had no Rust receipt. The active fast job median was 15.9 minutes; active factory/PR was 17.1. They are alternative owners, not two simultaneous Rust suites.

| Existing stage or row | Measured time | What executes and why |
| --- | --- | --- |
| Classify CI diff | Median 0.4 min, n=30 | Resolve comparison base and Rust/web/bridge signals |
| Fast row: no-build release rows | Median 0.5 min, n=30; range 0.1–1.7 | npm install plus changed-input structural checks and script tests |
| Rust toolchain/cache setup | Median 0.7 min on fast (n=13); 0.8 on factory/PR (n=15) | Zig/Rust/cache setup; occurs before the impact step |
| Active impact step | Median 13.8 min, n=28 | Validate target inventory, fetch failure history, compile and execute selected tests; conservative fallbacks often select workspace |
| Snapshot input router | Median 0.0 min on fast (n=13); 1.6 on factory/PR (n=15) | Separate named snapshot compile/run when its input paths change |
| Commander typecheck and unit tests | Median 0.6 min, n=10 across both rows | npm install, typecheck and tests on web inputs |
| Portable installer fixtures | Median ≤0.1 min, n=16 | Shell installer behavior; usually only factory/PR owner |
| Receipt publication / cache teardown | Included in job/run durations | Upload impact evidence and record cache stats; not another test suite |

The workflow routes both Rust owners to `ci-test-impact.py history` then `run`; see [.github/workflows/ci.yml:430 and :624](https://github.com/Richards-LLC/cassy/blob/6ae4d0f9a1d4a8af7d6b3bfa79bca6c25427a968/.github/workflows/ci.yml#L624). Deleting only the factory/PR job leaves the 13 broad fast-row runs intact.

The 28 receipts contain 16 scoped plans and 12 workspace fallbacks: seven unavailable previous-HEAD merge bases after rebasing/force-pushing, two shared build/test policy changes, and three unmapped paths (`.codex/config.toml`, `.codex/hooks.json`, `hub-web/package.json`). This is observed fallback behavior, not a recommendation to treat uncertain scope as safe. The selector deliberately widens on uncertainty and includes opaque CLI tests and module consumers: [ci-test-impact.py:118–210](https://github.com/Richards-LLC/cassy/blob/6ae4d0f9a1d4a8af7d6b3bfa79bca6c25427a968/scripts/ci-test-impact.py#L118).

Logs establish that the impact step is not just test execution:

| Run | Cargo compilation | Nextest execution | Selected tests |
| --- | ---: | ---: | ---: |
| [38989070161](https://github.com/Richards-LLC/cassy/actions/runs/38989070161) | 5m43s | 477.587s | 10258 |
| [38971270439](https://github.com/Richards-LLC/cassy/actions/runs/38971270439) | 4m14s | 431.061s | 13007 |
| [38966444549](https://github.com/Richards-LLC/cassy/actions/runs/38966444549) | 7m11s | 539.203s | 12975 |

The second run also compiles snapshot targets for 1m51s and runs 28 snapshot tests in 7.118s. These are three examples, not phase medians for all 30 deliveries.

## Whole-flow map and duplicated obligations

Current flow: focused worker tests/checks → factory push → classification → fast and factory/PR rows in parallel → close/verification/QA → epic merge → release preparation/PR → merge queue → main. Close gates consume evidence; they do not themselves compile. Their refusal can nevertheless force additional worker runs.

| Check and defect caught | Existing execution / gate | Duplication and proposed owner |
| --- | --- | --- |
| Named changed-behavior regression: broken handler or contract | Worker capped tests; broad branch selection; full queue suite | Worker owns focused development proof; queue owns integrated behavior. Remove the intervening broad branch replay |
| Reverse-dependent behavior, e.g. cas-pty prompt breaks cas-mux snapshot | Worker package receipts at close; CI dependency closure/workspace fallback; full queue suite | Remove mandatory unrelated per-package smoke receipts. Use existing focused impacted-consumer proof where known; queue owns exhaustive reverse-dependency coverage |
| Snapshot output mismatch | Worker relevant regression; impact-selected binary; separate snapshot router; queue suite | Include affected snapshot assertions in focused worker selection once. Remove separate branch snapshot replay when already covered; queue still validates final tree |
| Structural defects: call shapes, doctor snapshot grouping, version literals, markdown, migration registry, catalog | Changed-input no-build rows in lane; historical pre-push script exists; queue script tier covers many underlying contracts | Keep one factory owner; optional approved move to pre-push below. Queue's final-tree checks remain distinct integration proof |
| Web type errors / unit behavior | Worker iteration, selected lane web job, queue web job | Reuse focused worker final-tip proof on factory path; preserve ordinary PR fallback and final-tree web checks. Primary replay estimate does not subtract web work |
| Full Rust test graph and feature/build compatibility | Branch impact can fall back to workspace; queue archive/shards; queue preflight/no-default-features; macOS compile; doctests | Queue owns broad suite and platform/feature obligations. macOS compile, doctests and no-default-features are different defects; retain them |
| Installer behavior and CI/release machinery regressions | Branch installer fixture/changed script tests; full queue CI tiers | Early check only changed input contracts; full CI tiers proves integrated tooling. Do not move the entire multi-minute CI tiers target to pre-push |
| Pending/red lane result or stale docs-only success masking code | worktree_merge admission / CI watcher | Preserve existing tool refusal and critical red-on-merged wake. This is an evidence decision, not duplicate execution |
| Missing verification, user-path QA, unmerged delivery | Close / QA / merge evidence | Preserve. These establish review and delivery facts, not the Rust suite |

Sources: [receipt_scope.rs:85–130](https://github.com/Richards-LLC/cassy/blob/6ae4d0f9a1d4a8af7d6b3bfa79bca6c25427a968/cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/receipt_scope.rs#L85), [direct dependent selection :235](https://github.com/Richards-LLC/cassy/blob/6ae4d0f9a1d4a8af7d6b3bfa79bca6c25427a968/cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/receipt_scope.rs#L235), [worktree_ops.rs:113](https://github.com/Richards-LLC/cassy/blob/6ae4d0f9a1d4a8af7d6b3bfa79bca6c25427a968/cas-cli/src/mcp/tools/core/workflow/worktree_ops.rs#L113), CONTRIBUTING build policy, and the pinned workflow in the evidence bundle.

### The close gate can cost more than CI

cas-6c13 fixed a real escaped defect: cas-38e7 merged on pending CI and a dependent cas-mux prompt assertion went red seven minutes later, found at release nine hours later. Preserve its pending/red refusal. Its park gate, however, accepts any passing named test receipt in each direct dependent package for blast-radius tasks; it does not prove that arbitrary test covers the changed contract.

cas-8ea7 task notes give a concrete cost. Final head `ece40f969152a2785bd80a9e89e8288bd135cefb` had both CI rows green by **21:59:28Z** and real sandbox behavior proven. At **22:14Z**, close still refused six dependent packages: cas-mcp, cas-code, cas-mux, cas-pty, cas-factory and code-mode-mcp. The final consumer/check proof finished about **22:40Z**, at least **40.5 minutes after green CI**. This combines admission waiting, tests and compile check; it is not six tests' CPU time. Administrative re-anchoring and independent QA followed; integration closed at 23:04Z. Do not extrapolate this single incident into a 30-delivery close-gate median.

### The 3.61 merge queue and the late cheap failure

[PR #1250](https://github.com/Richards-LLC/cassy/pull/1250) opened at 01:35:01Z and merged at 02:36:14Z: **61m13s**. The first attempt [38991578507](https://github.com/Richards-LLC/cassy/actions/runs/38991578507) took 16m39s and failed only CI tiers. The second [38994862357](https://github.com/Richards-LLC/cassy/actions/runs/38994862357) took 10m29s, green at 02:35:57Z. The gap from first completion to second creation was **32m26s** for repair/re-enqueue. Queue attempts run parallel jobs: summing row times would overstate wall time.

| Queue job | Failed attempt | Successful attempt | Distinct proof |
| --- | ---: | ---: | --- |
| Preflight | 10.4 min | 4.2 min | Cargo check, no-default-features build, installer/ISA fixtures |
| CI tiers | 0.9 min (failed) | 5.6 min | CI/release/admission machinery contracts |
| Suite archive build | 11.5 min | 4.9 min | Compile full Rust test graph once, distribute archive |
| Three suite shards, longest | 4.1 min | 4.4 min | Execute full suite on archive; fan-in follows |
| Doctests | 5.2 min | 4.3 min | Rust documentation examples |
| macOS compile | 14.4 min | 6.0 min | Darwin-specific compile graph |
| Commander web | 7.9 min | 7.7 min | Typecheck, units, build/bundle, visual checks |
| Commander journeys, longest of six | 8.1 min | 8.1 min | Real web user paths; only when product inputs require them |

cas-2077 changed `hub-web/package.json` to add `node --test`. Its lane selected 12,975 Rust tests via an unmapped-path fallback, yet never exercised the cheap worker-memory package-script assertion. Queue log confirms `scripts/test-worker-memory.py:187`: **4294967296 != 2147483648**, conservative 4 GiB versus expected 2 GiB. `ci-script-tests-for-diff.py:62` diffs only scripts, and its HEAVY table defers the full worker-memory suite (~21s).

Proposed fix in the existing selector: map package script inputs to **the existing named weight-table assertion**, rather than the full suite. On this v47 base, `python3 scripts/test-worker-memory.py AdmissionTests.test_cas_4cb9_weights_follow_shell_and_actual_package_scripts` passed one test in **0.001s**, with no browser, npm invocation or Cargo. Retain full admission-concurrency behavior in queue. This is a measured cheap placement opportunity; it is not implemented here. The defect's initial failure log and passing current command are retained separately.

## Target design, placement and timing

1. Worker runs the changed behavior's real regression and relevant consumer/snapshot assertions once on the delivery tip, with existing capped receipts and checks. Interactive RED/GREEN work is development evidence, not another broad release gate. Preserve focused proof before handing off; do not require one arbitrary test in every dependent crate.
2. Factory CI keeps hosted classification, selected no-build contracts and cheap language-specific work. Both current Rust routes consume/reuse existing focused proof or omit broad Rust execution; neither may relabel no tests as a full-suite pass. Non-factory PRs without worker proof use a targeted hosted fallback in the existing stage. Unexpected scope requires review/focused proof, with exhaustive coverage in queue.
3. Close reads the existing delivered-tip proof, verification and QA. Let already-covered packages satisfy scope from credible machine evidence; defer exhaustive dependent coverage to queue. Preserve merge and stale-SHA protections. Do not add park/pre-merge compilation, fast-row, journey or assembly stages.
4. The queue remains the only gate to main: full Rust archive/shards, doctests, feature/platform compilation and input-selected web checks. A main push reuses the queue tree receipt. No separate assembly journey run or release PR queue cycle returns.
5. Route package-script changes to the 0.001s existing memory table check inside the cheap stage, without broad Rust fallback for the purpose of that contract. Keep the remaining memory-admission behavior at final integration.

**Pre-push decision:** cas-a229's October 16 14:37 note reports four first-push failures in nine deliveries from second-scale structural checks. Moving them before upload can avoid an entire failed CI cycle. If approved, move only changed-input **no-build** rows to the existing pre-push point and remove their factory-lane copy; keep normal PR validation. Never run Rust builds, the full CI-script tier, browsers or another suite there. Until that owner move is approved, leave the current lane owner in place. The worker-private installed pre-push hook inspected here enforces branch ownership only; the tracked `fast-rows-pre-push.sh` is not proof that these rows currently execute. Do not assume a hook or receipt is deployed just because its source exists.

The forecast deliberately takes no saving for moving cheap rows, web tests, toolchain setup, or consumer receipt waiting. It recomputes each job's finish after subtracting only its executed impact and snapshot steps, then takes the latest surviving job finish. Keep original starts, runner waits, commit→CI and CI-green→merge intervals. Result: **median lane 151s; commit→epic 1226.5s**. The modeled 2.5-minute lane is conditional on eliminating hosted broad compile/test replay, not merely changing its label or adding a narrower selector.

For immediate release with no batch wait: 1226.5 + 629 (observed successful queue) + 17 (green→main) = **1872.5s, or 31.2 minutes**. This is a model using one successful queue, not a measured release median or a guarantee. Reaching ≤30 requires another 72.5 seconds from handoff/commit→push or queue capacity; a five-minute commit→push budget instead of the observed 7.6-minute median is a proposed operating budget, not proven improvement. Under actual release batching, no ≤30-minute commit→main claim is supported: this sample's measured median is **268.7 minutes**.

Operator choices to record: approve or reject the ownership design; approve or reject moving no-build rows to pre-push; accept the conservative forecast and require real post-change timings before declaring the release goal met. Supervisor owns implementation scope only after that recorded decision. Roll back the routing if focused proof is not inspectable or queue escapes increase; do not automatically add a second gate.

## Method, evidence and limits

Population: the latest 30 distinct successful factory branches returned by the 250-run workflow query whose heads are ancestors of v46 `18821c4dee57544196352952d2c10e3f86ad172c`, limited to October 16 onward. Run creation spans **2026-10-16 20:28:52Z to 2026-10-17 02:00:27Z**. Deduplicate by branch; take the latest successful delivery. This is 30 delivered lane heads, not 30 randomly sampled first pushes. It excludes superseded attempts, cancelled runs and unmerged preparation (including cas-c423). Consequently it understates first-failure repair cycles; the separately analyzed queue failure demonstrates one such cost.

Evidence: GitHub jobs/steps for all 30 lanes and both queue attempts; impact receipts/plans for all 28 Rust lanes; three full logs for compile/execute phase examples; first failed queue log; PR #1250 metadata; first-parent integration ancestry and committer timestamps. [deliveries.csv](deliveries.csv) contains every run, branch, SHA, timestamp, integration commit, scope/reason and duration. [jobs.csv](jobs.csv) and [steps.csv](steps.csv) expose stage counts and distributions. Raw snapshots, collector, analyzer and paired forecast live in the task's published evidence bundle.

Collection commands: `gh run list --repo Richards-LLC/cassy --workflow ci.yml --limit 250 --json databaseId,headBranch,headSha,createdAt,startedAt,updatedAt,conclusion,status,event,displayTitle`; `gh api repos/Richards-LLC/cassy/actions/runs/<id>/jobs?per_page=100`; artifacts endpoint and artifact ZIP download for each run; `gh run view <id> --repo Richards-LLC/cassy --log`; `gh pr view 1250 --repo Richards-LLC/cassy --json number,title,createdAt,mergedAt,mergeCommit,commits`. The collector does not mutate remote state.

Lane wall = run createdAt→last non-skipped job completed_at, including hosted waits. Job and step durations use their own start/end, not run updatedAt. Epic merge = first commit on v46 first-parent history containing the lane head; commit→merge uses Git committer times (assumed host clocks consistent). Main time uses GitHub mergedAt. No task database was modified or directly queried for measurement. Close-wait attribution is from cas-8ea7's timestamped task notes, not an invented aggregate.

Source review base: v47/main `6ae4d0f9a1d4a8af7d6b3bfa79bca6c25427a968`, after the supervisor corrected the stale v47 ref. Historical workflow snapshot is pinned to v46 `18821c4d`; impact plans record the actual historical comparison bases. Current source can differ from historical run code; observations come from run receipts, not assumptions about the latest classifier.

Threats: survivor/recent-delivery bias; one release and only one successful queue; shared runner/cache contention; unknown decomposition of pre-push and gate waiting across most tasks; committer clock skew; future focused-proof ingestion cost. The forecast retains startup overhead but does not measure a new implementation. Approval, lower escape rates, real post-change medians, and the three consecutive clean on-budget releases required by cas-a229 are **not verified** here.

Decision record as of this report: **PENDING. Analysis approved; implementation not approved.** No CI/gate change is part of this delivery. Existing approvals in cas-a229 are historical context, not approval of this new target design.

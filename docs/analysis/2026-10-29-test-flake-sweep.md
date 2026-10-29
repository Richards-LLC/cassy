# Test flake sweep — static and history passes

Status: **passes 1–2 delivered; pass 3 NOT EXERCISED**. No test or production
code changed, no assertion weakened or deleted, and no test suite was run by
this audit. The release publication hold still applies.

Source: `75e37811c5e31471c51c8236f8bf2db3e48a3f41`. CI observation window:
October 15, 2026, 12:27 PM ET through October 29, 2026, 12:27 PM ET. History
outside that window is explicitly historical context. Line references below
belong to the source revision, including fixtures repaired on newer branches.

The canonical `FLAKE-SWEEP.md` and raw evidence live in the task artifact
directory for `cas-df00`. This repository copy records the reviewable result.

## Findings and order of work

Fix shared fixture isolation and deterministic observations first. Random
identities have the highest **verified repeated-test frequency** in this CI
window; asynchronous completion and resource ownership have repeatedly held
releases. A lexical hit is a candidate, not a failed test.

| Rank | Root pattern | Measured evidence / observed release cost | Follow-up |
| --- | --- | --- | --- |
| 1 | Process environment and fixture-store isolation | `cas-08e6` verifier: 3/5 module runs failed; corrective fixture: 10/10 runs, 400/400 selected cases passed. A live unguarded `CAS_AGENT_ROLE` writer remains in task-update tests. | `cas-49be` (P1) |
| 2 | Entropy and opaque-ID ordering | `memory_recent_contract` failed in 10 distinct CI job attempts; same head later passed. `cas-b0e7` held the 3.88 queue with a decimal-only SHA prefix, theoretical probability 1.455%. | `cas-0c89` (P2); shared Git defaults: existing `cas-d99b` |
| 3 | Completion inferred from elapsed time or timer advancement | WebCrypto sibling tests: baseline 4/200 and 137/200 failures in recorded stress controls. Queue-completion fixtures required two corrections. The 3.71 queue was ejected a second time by a 5-second browser expectation (`cas-d06f`). | `cas-9586` (P2) |
| 4 | Wall clocks, calendar boundaries and machine-speed assertions | Current tests still assert 50–500 ms upper bounds. Prior 6-second Git-history budget failed under load; recency evaluation crossed midnight. Current candidate failure rates are unknown. | `cas-65fd` (P1) |
| 5 | Socket, process and path ownership | Historical hub lock race: 5/25 before and 5/38 after an unrelated change; dead-port reuse required a Linux fix and a macOS follow-up. Shared process names and long scratch paths also caused assembly-only failures. | `cas-ead4` (P3, investigate first) |

`cas-d99b` already owns test-birth prevention and shared fixture defaults.
The five new tasks own concrete migrations or bounded investigations; they
do not duplicate that policy task. `cas-b2d8` already tracks the unresolved
reminder-expiration incident and should retain ownership of its diagnosis.

## Pass 1: static scan

Scanned **1,295 candidate files: 1,117 Rust and 178 web test/support files**.
The inventory includes integration tests, test support, source files named
for tests, balanced inline test blocks, Vitest tests and browser fixtures.
There are **8,234 lexical hits**, preserved individually with file, line,
pattern and source text in `STATIC-HITS.md` / `static-hits.json`.

| Pattern | Lexical hits |
| --- | ---: |
| Wall-clock reads | 2,715 |
| Sleeps, deadlines and elapsed checks | 1,269 |
| Git commits | 1,265 |
| Unordered maps / sets / iteration | 1,023 |
| HOME, CAS_ROOT and related root inputs | 832 |
| Browser settling / immediate observations | 623 |
| Random identities | 294 |
| Literal ports | 122 |
| Process environment / cwd mutation | 91 |

The scanner masks strings/comments for inline brace balancing, but pattern
matching itself remains lexical. Whole support files can contain non-test
code, string fixtures, ignored tests and unreferenced files. Multiline
expressions, imported aliases, generated code and indirect randomness can
escape lexical matching. This is coverage of the requested patterns, not a
proof that every latent nondeterministic test has been found.

### Environment and root selection

| Suspect | Evidence at source revision | Failure rate | Proposed fix / disposition |
| --- | --- | --- | --- |
| E1: task-update test changes authority without a guard | `cas-cli/src/mcp/tools/core/task/update.rs:3108`; writes `CAS_AGENT_ROLE` at `:3162`, removes at `:3171`; no fixture lock in this test, no unwind restoration. Other tests read that process variable. | Unknown; static interference risk, no new reproduction attempted. | `cas-49be`: acquire canonical guard and restore the original value, including on panic. |
| E2: manual save/restore around account-home overrides | `cas-cli/src/mcp/tools/service/factory_ops.rs:18596`, `:18751`, `:18974`, `:19211`, `:19316`. Legacy test lock participates, but restoration is manual. | Unknown; do not claim these are unlocked. | Migrate to guard-owned mutation, preserving lock scope and old values. |
| E3: inbox test manually removes process identity | `cas-cli/src/hooks/handlers/handlers_middle/factory_inbox.rs:1258`, `:1339`, `:1405`. The comments name the shared lock; removal is not restoration of an inherited value. | Unknown; guarded but fragile on unwind / inherited state. | Canonical guard; keep all inbox-surfacing assertions. |
| E4: proxy credential tests mutate process env directly | `crates/cas-mcp-proxy/src/lib.rs:3843`, `:3875`. Keys include PID plus sequence, reducing same-key collisions; cleanup follows fallible code. | Unknown; lower priority than E1. | Guard restoration or an injected resolver through the real configuration path. |
| E5: additional environment helpers | `crates/cas-store/src/agent_store/tests.rs:9` has its own mutex; `crates/cas-factory/tests/concurrent_factory_session_isolation.rs:14` has a local guard and one test. | Unknown; the one-test executable is not evidence of a cross-test race. | Check executable boundaries and reader participation before migrating. |
| E6: HOME-root differs from fixture store | `cas-cli/src/test_env_guard.rs:242` pins `HOME/.cas`; `cas-cli/src/ui/factory/daemon/runtime/shutdown_tests.rs:1343` creates that guard, then a separate store at `:1345`; status reads at `:1553`. | Historical verifier 3/5 failed; later corrective 7cdde8b6c: 0/10 failed module runs, 400/400 cases passed. | Already corrected by `cas-08e6`; use it as the control for `cas-d99b` / `cas-49be`, not a duplicate bug. |

Child-process `Command.env` does not mutate the parent process. The guard's
own raw writes are its implementation. A raw-write count therefore cannot
be reported as the number of rule-026 violations or racing tests. The current
source lint at `cas-cli/src/lib.rs:423` only scans specific spellings for
HOME, XDG_CONFIG_HOME and PATH; it does not catch E1's CAS_AGENT_ROLE.

### Entropy, Git identity and unordered results

| Suspect | Evidence at source revision | Failure rate | Proposed fix / disposition |
| --- | --- | --- | --- |
| I1: task fixture ID collision | `cas-cli/tests/fixtures/cas_instance.rs:41` draws 16 random bits, then inserts without collision handling. Caller examples: `cas-cli/tests/e2e/hook_e2e/jail_core.rs:34` and `:35`. | Two independent draws collide with probability 1/65,536; runtime incidence not measured. Some callers are ignored. | `cas-0c89`: deterministic per-fixture allocation; verify caller reachability first. |
| I2: memory fixture ID collision | `cas-cli/tests/fixtures/cas_instance.rs:48`, insertion at `:75`. Only 999 IDs per date. | Conditional on two same-date draws: 1/999. These memory workflow files are not imported by current `e2e/mod.rs`; no active CI failure claimed. | Remove entropy from the shared fixture; do not reactivate obsolete CLI tests to prove it. |
| I3: rule fixture ID collision | `cas-cli/tests/fixtures/cas_instance.rs:56`; 999 choices. | Same two-draw probability 1/999; current rule workflow module is unreferenced. | Same allocation fix; dormant risk, not a release-blocking observed failure. |
| I4: truncated Git SHA treated as a number | `cas-cli/tests/mcp_tools_test/task_tools/create_and_epic.rs:862` now pins dates; other commit candidates are listed in the raw inventory. | Historical `cas-b0e7`: decimal-only 9-character prefix probability `(10/16)^9`, about 1.455%; corrected fixture passed 30/30 clean-env runs. | Preserve the real Git path and token-shape assertion; shared deterministic Git setup belongs to `cas-d99b`. Ordinary ancestry assertions do not require a fixed SHA. |
| I5: random IDs assumed to increase with creation | Historical `memory_recent_contract::recent_declares_and_applies_stable_id_tie_break`. | 10 failed CI job attempts in window, not a population rate; recorded old two-ID ordering expectation about 50%. | Fixed by `cas-ceb6`; audit sibling ordered-ID expectations without changing the declared descending tie-break contract. |
| M1: unordered maps in protocol snapshots | `crates/cas-factory-protocol/tests/protocol_tests/enums_and_nested.rs:311`, with encode/decode followed by structural equality. | No defect confirmed. | Cleared sampled case: HashMap equality is order-independent. Do not sort or replace it just because the lexical scan matched. All remaining hits stay in the inventory for follow-up triage. |

### Clock, speed and asynchronous completion

| Suspect | Evidence at source revision | Failure rate | Proposed fix / disposition |
| --- | --- | --- | --- |
| T1: blocked writer must finish calls within 50 ms | `crates/cas-pty/src/input_queue.rs:274`. Existing fixture already has a gate, capacity and byte-order assertions. | Unknown. | `cas-65fd`: prove nonblocking return while the writer is held, with a separate bounded hang guard. Keep capacity and byte-order coverage. |
| T2: 1,000 seeks must finish within 100 ms | `crates/cas-recording/src/reader.rs:709`. | Unknown. | Controlled work metric or performance harness with explicit machine assumptions; preserve seek behavior assertions. |
| T3: search 1,000 symbols within 500 ms | `crates/cas-search/tests/code_search_integration_cases/tests.rs:348`; existing fetch counters follow it. | Unknown. | Assert the actual bounded-fetch work through those counters and preserve result correctness. |
| T4: mux async begin / redirect deadlines | `crates/cas-mux/src/mux_tests/tests.rs:3795` (<100 ms), `:3644` (<400 ms). | Unknown. | Observe injection progress / held floor using a controlled clock; retain the no-wait and harness-specific behavior. |
| T5: two 400 ms upstream timeouts within 700 ms | `crates/cas-mcp-proxy/src/lib.rs:4440`; comment documents previous 55 ms slack flake and its wider replacement. | Current rate unknown; historical `cas-2b7e` confirmed earlier failure. | Observe both upstreams starting before either finishes; do not keep increasing wall-clock slack as the only proof. |
| T6: today parsed before another clock read | `crates/cas-core/src/search/temporal/tests.rs:59`–`:60`. | Unknown; vulnerable if calls straddle UTC midnight. | Inject one explicit boundary instant into parsing and expectation. |
| T7: malformed timestamp fallback compared to later now | `crates/cas-store/src/knowledge_store.rs:3271`, absolute difference <5 seconds. | Unknown; host suspension or scheduling delay is sufficient in principle. | Controlled fallback clock while preserving the malformed-input fallback contract. |
| A1: one event-loop turn assumed to drain stopped stream | `hub-web/src/connection-lifecycle.test.ts:171`. | Unknown; lower-confidence candidate. | `cas-9586`: first prove the stream-completion ordering; if necessary await that observation. |
| A2: focus expected after 5 ms | `hub-web/src/session-thread-identity.test.ts:371`; mock completion begins with a zero-delay timer at `:358`. | Unknown; timers may already order this safely. | Clear by tracing the actual mock chain or await focus/completion; do not label it a measured flake. |
| A3: account-sheet one-turn settle | `hub-web/src/accounts-panel.test.ts:11`. | Unknown; synchronous mocked promises may make it sufficient. | Verify delayed completion through the actual sheet path before changing it. |
| A4: paint completion raced against 250 ms | `hub-web/e2e/journeys/journey.ts:282`; a held browser clock deliberately prevents animation frames. | Unknown; intentional escape, not a confirmed race. | Validate captured state explicitly under held/advancing clocks; preserve the held-clock escape. |

`hub-web/e2e/journeys/real-hub.journey.ts:334` deliberately holds a 60-second
fault. That is fault duration, not a sleep used to settle an assertion.
Likewise, bounded condition polling and a generous hang guard are different
from assuming work has completed after one sleep.

### Ports, processes and filesystem resources

| Suspect | Evidence at source revision | Failure rate | Proposed fix / disposition |
| --- | --- | --- | --- |
| R1: checkout-derived web ports can collide | `hub-web/e2e/checkout-ports.mjs:28`–`:31` maps paths into 6,384 pairs; identical checkout paths always use the same pair. | Unknown; admission and explicit caller overrides may already prevent concurrent collisions. | `cas-ead4`: trace actual reservation/serialization. Existing `reuseExistingServer: false` fails loudly instead of using another build. |
| R2: literal localhost ports | 122 lexical hits, including configuration values, expected origins and authorization fixtures. | No newly confirmed fixed listener defect. | Do not report every URL as a bound-port collision. Live proxy test listener at `crates/cas-mcp-proxy/src/lib.rs:3589` uses port 0. |

## Pass 2: CI and Cassy history

The GitHub all-runs endpoint capped the first query at 1,000 results. Daily
paginated queries recovered **4,273 CI / Release Prebuild runs** in the
observation window. All 3,915 distinct commit trees were resolved from local
Git objects or read-only GitHub commit metadata. A single HTTP 502 during
job retrieval was recovered; no test was rerun for this audit.

| CI population | Success | Failure | Cancelled |
| --- | ---: | ---: | ---: |
| CI push / lane | 3,750 | 167 | 104 |
| CI pull request | 103 | 3 | 0 |
| CI merge group | 42 | 29 | 0 |
| Release Prebuild merge group | 70 | 1 | 0 |
| CI scheduled / manual | 4 | 0 | 0 |

Retrieved job/step records for all **200 final-failure runs plus relevant
reruns**: 225 run records in that initial set. Selected 237 failed
non-aggregate jobs and preserved 236 logs, plus successful control logs.
Job 116895169616 (`macOS Check`, run 38877186940) has no steps and its log
returns HTTP 404; its cause is NOT VERIFIED. The failure discovery
index contains 1,478 distinct Rust/Vitest test names across 166 test-bearing
failed job attempts. That index includes intentional red commits and
deterministic regressions: it is **not 1,478 flakes**. Browser journey
failures have a separate named index.

There are **33 workflow/tree groups with mixed conclusions**. A green lane
that skipped Rust or journeys does not establish that the failed test passed.
Comparing available positive execution logs in those groups produced no
additional directly matched failing-test/pass pair. Eight failed job attempts
had a same-run later green job; two later jobs skipped Rust, leaving these
**six directly supported failed attempts across five heads**:

| Test / pattern | Same-head failure → executed pass | Evidence and rate interpretation |
| --- | --- | --- |
| WebCrypto retry-signing completion (`cas-1b81`) | [38781758179](https://github.com/Richards-LLC/cassy/actions/runs/38781758179), attempts 1 and 2 failed; attempt 3 passed | Head `13e0ad4eb36a50aa51eca68fb1dd7985ab88b9f2`; successful job 116590145334 ran 1,330 Vitest cases. Selected reruns 2/3 failed; not an unbiased incidence estimate. |
| Inbox publication (`cas-c312`) | [38948837325](https://github.com/Richards-LLC/cassy/actions/runs/38948837325), attempt 1 failed → 2 passed | Head `03a0ec6949416f048af5e563604ff270bf7d6c2c`; successful job 117142098678 ran 12,933 Rust cases. Failure was EOF while reading non-atomic inbox write. |
| Opaque-ID tie-break (`cas-ceb6`) | [38929147147](https://github.com/Richards-LLC/cassy/actions/runs/38929147147), attempt 1 failed → 2 passed | Head `574d765082f88e12d81b8ccba8c6151795c05413`; successful job 117075641017 ran 12,823 Rust cases. Same test failed in 10 distinct job attempts in the broader window. |
| Queue commit observation (`cas-c4b3`) | [39046477304](https://github.com/Richards-LLC/cassy/actions/runs/39046477304), attempt 1 failed → 2 passed | Head `37faf15d408678595b51212ca9f74e711d14b05b`; successful job 117439693132 ran 10,975 Rust cases. Writer-entry flag preceded committed queue progress. |
| Dead-port reservation (`cas-f0a8`) | [39271623903](https://github.com/Richards-LLC/cassy/actions/runs/39271623903), attempt 1 failed → 2 passed | Head `9a3018c691245de7be8f15378ec12e5dd44542d4`; successful job 118134696637 ran 13,097 Rust cases, one leaky. Passing did not erase the leak warning. |

Excluded as test-pass evidence: runs 39278300482 and 39278529048 later became
green but their `Run change-scoped tests with impact evidence` steps were
**skipped**. One involved hub-stop/peer routing; the other involved builtin
wording contracts. Their original failures remain in `CI-FAILURES.md`.
Compile-only Clippy failures, docs lint, dist drift and event-dependent tier
selection similarly do not establish test nondeterminism.

### Recorded incident ledger, grouped by cause

Rates below are measurements reported in the original Cassy records, except
where explicitly mathematical. They are not fresh audit executions. The full
history inventory preserves older incidents separately from the CI window.

| Pattern | Incident / evidence | Failure rate and observed cost | Disposition |
| --- | --- | --- | --- |
| Env/root mismatch | `cas-08e6`, verifier at d4d5ff883, corrective 7cdde8b6c | 3/5 failed module runs; review rejection during 3.89 preparation | Corrected; 400/400 module cases passed; native product proof retained separately. |
| Env isolation | Historical `cas-ce7e`: duplicated HOME helpers and two mutexes | Failure then pass on rerun; population rate unknown | Closed; residual writers now belong to `cas-49be`. |
| Entropy/order | `cas-ceb6` | 10 failed CI attempts; old ordering assumption about 50% for two generated IDs | Closed; independently sort expected opaque IDs. |
| Entropy/parser | `cas-b0e7`, numeric SHA 940220173… | Theoretical 1.455%; second 3.88 queue ejection | Corrected; original warning assertions retained, 30/30 clean-env passes. |
| Entropy/parser | Historical `cas-a819`, 12-digit abbreviated stash SHA | Theoretical 0.355%; held 2.69.0 release | Closed; force object parsing with `^0`. |
| Native async / fake timer | `cas-ec55` | 4/200 stress failures before, 0/200 after; docs-only lane red | Closed; slow-native-digest control preserved. |
| Native async / fake timer | `cas-1b81` | 137/200 stress failures before, 0/200 after; release-blocking sibling | Closed; bounded actual-state observation. |
| Queue completion | `cas-44528` then `cas-c4b3` | Same-head red→green in CI; exact population rate unknown | Closed; gate writer and observe committed queue state, not entry into writer. |
| File publication | `cas-c312` | One same-head red→green pair; 200/200 recorded stress passes after fix | Closed; atomic temp-file rename plus stable lock. |
| Shared test state | `cas-a8dc` | Base failed 3/3 parallel runs, passed serially; repaired 10/10 parallel runs | Closed; do not mistake isolation-only passes for concurrency proof. |
| Calendar clock | Historical `cas-f382a` | Same-tree retrieval metric 0.150787 vs 0.149894; rate unknown | Closed; one evaluation `as_of`, exact equality retained. |
| Calendar clock | Historical `cas-1971` | Time-of-day journey risk; no unbiased rate | Closed; pinned epoch/timezone, advancing clock rather than a fully frozen one. |
| Wall-time budget | `cas-241c` | 6-second Git-history budget failed under load | Closed; actual Git-call count proves bounded work. |
| Wall-time budget | `cas-1518` (earlier on window start date) | Tight RTT upper bounds failed on loaded hosted runners | Closed; 20/20 recorded two-CPU runs, lower bounds retained. |
| Dead port / platform | `cas-f0a8`, follow-up `cas-63ff` | Linux same-head red→green; macOS reserved non-listener timed out rather than refusing | Closed; established socket ownership, with platform-specific proof. |
| Lock race | `cas-036b` | 5/25 and 5/38 failures in recorded controls; repeated hub restart failure | Closed; bounded real lock contention fix, 50/50 follow-up passes. |
| Process identity | `cas-675a` | Shared `relay-worker` name deterministically failed with live matching host process | Closed; unique fixture name, 20/20 loaded recorded passes. |
| Filesystem / descriptor context | `cas-ad72`, `cas-674f` | Four assembly-only long-path/high-FD failures | Closed; explicit deep-path controls and sanctioned short socket fixture. |
| Async launch readiness | `cas-1474`, correction `cas-470e1` | Initial root cause unverified; later delayed-consumer red reproduced it | Corrected; actual readiness/submit observations, macOS canonical path. |
| Host-dependent pagination | `cas-fe7d` | Same tree: 238/240 rows under load, standalone pass | Closed; transient host scans follow stable store-backed inventory. Product bug, not merely a test fix. |
| Browser refresh | `cas-d06f`, run 39694702194 | One named journey exceeded 5 seconds; second 3.71 queue ejection | Historical repair; include in async systemic audit, do not infer current recurrence. |
| Browser send state | Historical `cas-cff2` | 1/6 at four workers plus two other affected runs | Closed; product/fixture distinction remains necessary. |
| Reminder expiry | `cas-b2d8`, run 40615407971 job 122580953212 | Assembly 322/323 in recorded local selection; CI named failure also observed | OPEN; root cause and flake status NOT VERIFIED. Do not merge into generic timeout work. |

Search found 57 flake-title task results, six nondeterminism task results and
19 flake memory results. Because snippets were insufficient for root-cause
classification, read-only SQLite `mode=ro` exports expanded discovery to
896 task-field matches and 43 memory matches. Most task-field matches are
quoted context, QA wrappers or coordination history. The focused title index
has 77 historical flake/nondeterminism records; all are discoverable in the
artifact inventory. Keyword matches alone are never ranked as confirmed bugs.

## Pass 3 and completion boundary

**NOT EXERCISED: Rust full suite 0/5; Commander journeys 0/3.** No supervisor
message has authorized starting after publication of v3.89.0. The worker
execution policy also reserves full Rust suites and `--full` journeys for
the supervisor / merge queue. The supervisor must own or explicitly arrange
this phase after the release hold ends; this audit does not bypass the runner.

Run against one immutable tree, with no retries, retaining every original
failure, runner concurrency, actual load measurements, test counts, duration,
logs and journey traces. A later isolated pass is a diagnostic control, not
permission to erase an earlier failure. Record failures per individual test
and denominator separately from whole-run failure rate. Do not change source
between repetitions or silently omit an expensive harness.

Every live journey is currently **NOT EXERCISED: read-only audit; release
hold**. `JOURNEY-STATUS.md` records one row per registry journey. Passes 1–2
support prioritizing repairs; they do not establish a clean five-run suite,
eliminate all latent flakes or complete the original three-pass task.

## Evidence inventory and acceptance checks

- `FLAKE-SWEEP.md`: this ledger, with explicit pending phase.
- `STATIC-HITS.md`, `static-hits.json`, `static-files.json`, `scan.py`: each
  lexical hit and repeatable scope/method.
- `ci-window.json`, `ci-trees.json`, `ci-tree-recovery.json`, daily query files:
  bounded run census and tree identities; `mine_ci.py` records queries.
- `jobs-*.json`, `job-*.log`: raw job/step metadata, original failed logs and
  positive controls. Logs are data; terminal escapes are stripped for analysis.
- `CI-FAILURES.md`, `ci-failures.json`, `ci-journey-failures.json`: named
  discovery index; repeated log summaries deduplicated per failed attempt.
- `ci-mixed-trees.json`, `ci-mixed-success-jobs.json`: mixed-conclusion groups
  and positive execution selection, with skipped-test caveats.
- `CASSY-HISTORY.md`, focused history index and read-only raw exports:
  original causes, fix outcomes, failed approaches and uncertainties.
- `JOURNEY-STATUS.md`: registry coverage marked NOT EXERCISED.

Acceptance for this partial delivery: source references exist at the audited
revision; inventory counts reconcile with JSON; every failed/rerun run has
job metadata; every selected failed job has a retained log or the explicit HTTP 404 gap; all five fix tasks
belong to `cas-cde3`; Markdown lint and `git diff --check` pass. There are no
changed production functions, test modules, browser inputs, builtin catalogs,
dispatch surfaces or migration contracts to execute for this documentation
change. Full dynamic evidence remains explicitly pending.

design: DESIGN.md@b43940e43

# Cassy needs targeted repairs to failure boundaries and lifecycle interfaces

**Verdict — high confidence in the cited code facts; moderate confidence in repository-wide prioritization.** This audit found reproducible edge-case failures and substantial lifecycle coupling. Prioritize durable sync intent, valid generated frontmatter, and Slack session identity; follow with a clearer close service. Broad deletion or a repository rewrite is not supported by this evidence.

| Overview | Assessment |
| --- | --- |
| Question | Where does cas-src contain concrete correctness, maintainability, performance, or operational debt? |
| Verdict | Repair the verified boundaries first; deepen lifecycle interfaces incrementally. |
| Confidence | High for reproduced results and inspected control flow; medium for predicted production impact and effort. |
| Scope | Whole tracked-file inventory, followed by risk-based source sampling across Rust, shell, YAML, Markdown generators, hub-web and slack-bridge. This is not a line-by-line review of every file. |
| Baseline | `bbc1629b85ba891e248f6b328d9b78a9a45beaba`, both checkout HEAD and origin/main at audit start; commit time `2026-09-04T22:12:34Z`. |
| Date / author | 2026-09-04 UTC / Codex, fair-swan-9, practitioner Investigation/Diagnostic. |
| Changes | This delivery adds only the Markdown and HTML report. Product files are unchanged. |

## Ranked evidence

P1 means address before relying more heavily on the affected workflow; P2 means bounded follow-up; P3 means opportunistic cleanup. Priorities are audit judgments, not incident severity. Effort is an estimate: S is a localized patch and focused tests; M crosses a component seam; L requires staged migration and integration proof. Confidence concerns the stated finding, not the frequency of incidents. Every row includes a concrete falsifier; none is a claim that an incident occurred in production.

| Rank / class | Finding and verified source | Impact and reachability | Confidence | Effort | Falsifier |
| --- | --- | --- | --- | --- | --- |
| 1 · P1 · correctness / error handling | Task persistence discards outbox errors. `cas-cli/src/store/syncing_task.rs:53`, `:80`, `:229`; `cas-cli/src/cloud/sync_queue/queue_ops.rs:117`. | A successful local write can have no retryable sync intent. Active when cloud wrapping is enabled. | High control-flow; failure not injected into Rust wrapper. | M | Failed enqueue leaves a durable repair marker or is surfaced to the caller on every path. |
| 2 · P1 · correctness / DRY | Duplicated skill YAML escaping produces invalid frontmatter. `cas-cli/src/sync/skills.rs:432`; `crates/cas-core/src/sync/skills.rs:379`. | Ordinary descriptions containing a Windows path and colon can become unreadable YAML. Both implementations have live consumers. | High; extracted Rust helper reproduced. | S–M | Generated description round-trips unchanged through the actual YAML reader. |
| 3 · P2 · correctness / domain identity | Slack session IDs truncate timestamp bytes. `slack-bridge/src/daemon.ts:87`, `:98`, `:126`. | Distinct threads can request the same session ID; bookkeeping also loses established sessions on daemon restart. Active message path. | High collision; restart consequence inferred from source. | S–M | Distinct scoped thread keys map to distinct IDs, and restart recovers existing-session state. |
| 4 · P2 · architecture / operability | Slack direct execution coexists with an unwired factory/SSE design. `slack-bridge/src/daemon-main.ts:151`, `:193`; `slack-bridge/src/daemon.ts:101`, `:118`, `:201`. | Commands launch a permission-skipping child outside factory orchestration; handler calls are not serialized per thread. Dormant subscription logic obscures the actual contract. | High code facts; no live Slack or harness invocation. | M | Production selects and tests an explicit execution policy with per-thread serialization and recovery. |
| 5 · P2 · build / I/O | Build script watches directory paths beneath a worktree `.git` file. `cas-cli/build.rs:49`, `:50`. | Missing watched paths invalidate unchanged builds; version metadata uses the wrong Git topology. | High analogous Cargo probe; full cas compile cost unmeasured. | S | Two unchanged worktree builds keep the build script fresh while a real ref change updates provenance. |
| 6 · P2 · architecture / testability | Task close combines lifecycle policy, evidence collection, persistence and presentation. `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs:1964`, `:2740`, `:3243`, `:4107`, `:4367`. | A change must preserve eight caller obligations across five dependency categories; effects and transport errors are interleaved. | High structural; no latency or regression-rate claim. | L | A transport-independent close interface owns the invariants and supports real-store integration tests without MCP/environment setup. |
| 7 · P2 · correctness / CI | Classifier reports `empty` after failed `git diff`. `scripts/classify-ci-diff.sh:17`; `.github/actions/classify-required-diff/action.yml:50`, `:68`. | Reproduced false success in helper. Normal action guards invalid bases first; a subsequent diff failure can still be misclassified. | High helper result; narrower CI exposure. | S | A Git failure returns nonzero or conservatively reports `rust-touched`. |
| 8 · P2 · latent correctness / tests | Upload staging rejects a symlink after overwriting its target. `slack-bridge/src/file-handler.ts:123`, `:128`; `slack-bridge/src/file-handler.test.ts:153`. | Outside fixture data changes despite failure result. Currently test-only code, not a demonstrated remote upload vulnerability. | High native Node reproduction; dormant reachability. | S–M | Rejection preserves outside bytes and rejects symlinked ancestors before any write. |
| 9 · P2 · architecture / complexity | Commander rendering retains a large implicit state interface. `hub-web/src/main.ts:1626`, `:1648`, `:1685`, `:1746`, `:1863`. | UI policy and DOM shell construction share mutable module state; review and targeted testing require knowing that state. | High source/complexity; no measured rendering slowdown. | M–L | A bounded view model covers the render inputs and DOM tests exercise its observable transitions. |
| 10 · P3 · DRY / I/O ownership | IndexedDB transaction cleanup is duplicated. `hub-web/src/storage.ts:19`, `:29`, `:204`, `:213`. | Abort, error, connection closure and settlement rules must evolve together in two paths. | High Fallow clone and source match. | S–M | A shared lifecycle primitive owns both paths while preserving atomic read-modify-write. |
| 11 · P3 · operability / distribution | Shell installer installs and de-quarantines a downloaded archive without digest verification. `scripts/cas-install.sh:225`, `:238`, `:249`, `:257`. | Installer trust is HTTPS plus the release asset; package substitution is not checked against an independent receipt. | High inspected flow; no compromise alleged. | M | A trusted digest/signature/attestation is verified before extraction and executable replacement. |

## Reasoning chain

The verdict follows three distinct evidence classes. **Reproduced behavior:** invalid skill YAML, colliding Slack IDs, classifier false success, and a dormant staging overwrite; an analogous Cargo fixture demonstrates the worktree invalidation mechanism. **Inspected active control flow:** sync intent can be discarded, Slack starts a direct child, and the installer has no artifact-integrity step. **Structural debt:** close orchestration, renderer inputs, and repeated transaction cleanup expose avoidable knowledge to maintainers. These classes are separated so a static complexity score never poses as a runtime defect.

The pattern is incomplete ownership of an operation: a local write succeeds before sync intent is durable; staging decides whether a write was permitted after performing it; a shell helper loses its producer's error; and close requires knowledge of multiple state machines through one transport handler. The justified response is to strengthen those interfaces and their observable guarantees. The inventory and false-positive checks do not support indiscriminate abstraction removal.

### 1. Persist sync intent with the task mutation

`SyncingTaskStore::add` commits through `inner.add`, reloads the stored task, calls a void `queue_upsert`, then returns `Ok(())` (`cas-cli/src/store/syncing_task.rs:229–233`). Personal enqueue uses `let _ = self.queue.enqueue(...)` at line 80; team enqueue does the same at line 68. `SyncQueue` executes fallible SQLite SQL and propagates its error internally (`cas-cli/src/cloud/sync_queue/queue_ops.rs:105–140`), which the wrapper then discards. `update` repeats the order at `syncing_task.rs:265–287`.

This is an actual active seam: `cas-cli/src/store/detect.rs:406–422` enables the wrapper for logged-in configuration, but also silently falls back to the base store if opening the queue fails. Personal push consumes pending queue rows (`cas-cli/src/cloud/syncer/push.rs:63`); its retry mechanism cannot retry an intent that never reached the queue. This audit did not inject a SQLite failure into the complete Rust wrapper and does not claim measured data loss. A later mutation may enqueue a fresh state, but that is not a guarantee for an otherwise untouched task.

**Remediation direction:** make local mutation plus outbox intent one durable operation where possible. Otherwise persist a reconciliation marker and explicitly report degraded sync without pretending the local commit failed. Simply adding `?` after the committed write creates an ambiguous retry contract. Pin success, queue failure, team/personal partial failure, and restart reconciliation through the store interface with real SQLite.

### 2. Replace duplicated scalar escaping with a proven serializer

The two skill writers contain the same `escape_yaml` function. When a string contains a colon, the helper surrounds it with double quotes but escapes only double quotes and newlines; backslashes remain raw (`cas-cli/src/sync/skills.rs:432–437`; `crates/cas-core/src/sync/skills.rs:379–384`). Compiling the actual extracted Rust function and passing `Use C:\project: inspect` produced:

```text
description: "Use C:\project: inspect"
YAML parser: found unknown escape character 'p'
```

This is not dead legacy duplication: the CLI imports its local `SkillSyncer` (`cas-cli/src/cli/update.rs:36`, `:1799`), while the MCP server imports `cas_core::SkillSyncer` (`cas-cli/src/mcp/server/mod.rs:18`). The CLI writer emits the escaped description at `cas-cli/src/sync/skills.rs:71` and writes the generated document at line 310. The spec writer already escapes backslashes (`crates/cas-core/src/sync/specs.rs:270–282`), demonstrating drift between nearby policies; it is not evidence that the skill writers are safe.

**Remediation direction:** share scalar serialization and round-trip tests, including backslashes, quotes, YAML implicit scalars, leading punctuation, newlines and Unicode. Keep differing harness metadata policy explicit. The deletion test favors deleting duplicate escaping, not deleting one entire skill implementation before comparing its behavior. The reproduced YAML rejection used PyYAML; no harness parser was invoked, so user-visible rejection remains a protocol-level consequence to confirm in integration.

### 3. Scope and persist Slack thread identity

`threadTsToSessionId` converts the timestamp string to hex and keeps only 32 hex characters, which preserves only 16 original ASCII bytes (`slack-bridge/src/daemon.ts:87–91`). The following two inputs yielded exactly the same ID when the function was extracted from the inspected source and executed:

| Input timestamp | Generated session ID |
| --- | --- |
| `1757020000.123451` | `31373537-3032-3030-3030-2e3132333435` |
| `1757020000.123459` | `31373537-3032-3030-3030-2e3132333435` |

Source: `thread-session-proof.json`, source function at the baseline commit; synthetic inputs, not actual Slack messages. The raw timestamp alone also omits channel and project scope. Router message construction carries both (`slack-bridge/src/router.ts:120–127`), so that scope is available.

The in-memory `threadSessions` set (`daemon.ts:81`) decides new versus resume at line 99 and marks a thread started immediately after spawn at line 126, before success. A failed spawn or restart can therefore disagree with persisted harness session state. This is a source-derived recovery risk, not a reproduced harness failure. **Remediation direction:** derive or persist an ID from the full scoped key, record successful establishment, serialize same-thread requests, and test collisions, startup failure and restart recovery using an injected process runner.

### 4. Give the bridge one explicit execution contract

The live handler calls `injectMessage` directly (`slack-bridge/src/daemon-main.ts:193`), and that implementation starts `claude` with `--dangerously-skip-permissions`, fixed effort, fixed turn limit and fixed timeout (`slack-bridge/src/daemon.ts:101–123`). These are operational policy choices embedded in the transport adapter. Socket handling invokes promises independently (`daemon.ts:195–203`), so multiple messages may spawn overlapping work for the same thread.

Meanwhile `ensureSubscription` at `daemon-main.ts:151` has no call site in that file or the repository search; its subscription map is populated only inside that function. SSE routing and shutdown machinery thus describe a path the active handler does not enter. Router channel and user allowlists are real (`router.ts:102–117`) and explicitly rule out a claim that arbitrary Slack users can invoke this path. No live Slack API calls, messages, or Claude child invocations were performed for this audit.

**Remediation direction:** decide whether the supported operation is factory dispatch or isolated direct execution, then expose that choice through an executor interface with explicit limits, cancellation, serialization and resume outcomes. An adapter seam is justified if both execution modes remain supported; if only direct execution is intended, deleting the dormant SSE orchestration reduces the interface without redistributing useful behavior. Avoid keeping an unused second architecture “for flexibility.”

### 5. Resolve real Git paths before registering build inputs

`cas-cli/build.rs:49–50` registers `../.git/HEAD` and `../.git/index`. This checkout's `.git` is a file, as in a linked Git worktree. Those descendants do not exist. A dependency-free Cargo fixture using the exact two directives and an analogous `.git` file was checked twice. The second unchanged check reported:

```text
Dirty audit-build-fixture: the file `../.git/HEAD` is missing
Compiling audit-build-fixture
Running .../build-script-build
Finished `dev` profile ...
```

Source: `build-probe-second.log`. Fixture elapsed times are not a Cassy rebuild benchmark or predicted saving. This mechanism is relevant to the project's isolated worktrees even though hardlink target seeding and same-path sccache reduce other costs. Missing optional `.env` watches in the same script may also invalidate builds; they were not separately benchmarked here.

**Remediation direction:** resolve the worktree-aware paths with Git, watch existing relevant files including the symbolic branch reference, and define how dirty-state changes should refresh embedded metadata. Test normal checkout and linked worktree behavior on two unchanged builds and after a commit. Do not remove provenance simply to make Cargo report “fresh.”

### 6. Deepen the close service, preserving its real policy

`cas_task_close_with_completion` occupies lines 1964–4530, **2,567 physical lines including comments and whitespace**. This interval is the handler itself, not its colocated tests. The containing file has 23,381 lines, which includes substantial tests and is not a complexity metric. The reason to restructure is the interface burden, not either line count.

Eight caller obligations are visible: **identity**, **task/epic ownership**, **task type and execution-depth contract**, **repository/target selection**, **content attribution**, **verification-cycle binding**, **override authority**, and **receipt/retry interpretation**. These are audit groupings, not an automated count of parameters. The public request starts at `cas-cli/src/mcp/tools/types/task.rs:173`; three more optional inputs appear at `close_ops.rs:1967–1969`. Environment-derived identity appears at `:2261`, policy at `:2740`, Git merge validation at `:2647`, dispatch creation at `:3243`, final task persistence at `:4107`, and lease release at `:4367`.

| Dependency category | Evidence / current responsibility | Proposed test boundary |
| --- | --- | --- |
| In-process domain rules and types | `close_ops.rs:2259`, `:2740`: ownership and harness verification policy | Pure policy outcomes with explicit resolved context. |
| Local SQLite stores and queues | `close_ops.rs:1971`, `:3243`, `:4107`, `:4367`: task, dispatch and lease state | Real temporary stores; verify committed and reverse/retry states. |
| Local Git and filesystem | `close_ops.rs:2647`, `:6216`: merge conflict/evidence checks | Real disposable repositories, explicit evidence snapshot. |
| Process environment and configuration | `close_ops.rs:2262`, `:2739`: caller identity and mode | Resolve once at transport boundary and inject typed context. |
| Local daemon transport | `close_ops.rs:3226–3232`: notification event emission | Explicit post-commit effects with observable delivery/degradation. |

**Deletion test:** deleting this handler would make policy complexity reappear across callers. It has earned its responsibility; the seam has insufficient locality. Keep the domain operation and separate evidence collection, transition decision, durable application and response rendering behind a small application-service interface. A mere file split would not reduce caller knowledge. A generic configurable gate/plugin engine would add ordering and configuration facts without a demonstrated second use case. Prefer staged extraction with existing integration contracts, including rejected close, verification timeout, reopen and idempotent retry. No claim of missing tests is made: `cas-cli/tests/mcp_tools_test/task_tools/verification_flow.rs` itself contains 9,183 physical lines in the inventory.

### 7. Preserve producer failures in the CI classifier

The classifier reads `git diff` through process substitution (`scripts/classify-ci-diff.sh:17–19`). Bash `set -euo pipefail` does not make the enclosing loop fail when that producer fails. With a nonexistent base, the helper printed `empty` and exited zero, alongside Git's fatal error. Exact command:

```bash
bash scripts/classify-ci-diff.sh audit-nonexistent-base HEAD
# exit 0; stdout: empty; stderr: fatal: ambiguous argument ...
```

The normal composite action calls `git merge-base` first and falls back to the Rust tier if it fails (`.github/actions/classify-required-diff/action.yml:50–54`). Therefore the invalid-base fixture does **not** demonstrate that normal CI skips checks for an invalid base. The remaining concern is the helper's broken contract and failures after successful base resolution: the action trusts a returned `empty` at lines 68–70. **Remediation direction:** capture the diff with an explicitly checked status before iterating; test invalid refs and an injected failing Git executable after base resolution. Preserve the documented scoped/full CI policy.

### 8. Fix staging before wiring it into production

Native Node 24 imported `stageFile` directly from the TypeScript source. In a disposable directory, `uploads/report.txt` was a symlink to `outside.txt`. After staging, the result was `ok: false`, while `outside.txt` contained `OVERWRITTEN`. The symlink remained because the ESM error path calls CommonJS `require`, whose failure is caught and replaced with “Failed to verify staged file” (`slack-bridge/src/file-handler.ts:132–144`).

The defect is in ordering: `writeFileSync` at line 123 follows the link before `lstatSync` at line 128. The existing test explicitly acknowledges the write but checks only rejection (`slack-bridge/src/file-handler.test.ts:153–157`), so it does not enforce the intended non-modification guarantee. No test-suite pass is asserted; the native import probe differs from Vitest's module handling.

**Reachability limit:** source searches show this helper is referenced by its tests, with no active router/daemon call. Filename sanitization is present, and the router drops non-regular message subtypes (`router.ts:94`). Thus this is a verified latent file-integrity defect, not evidence of an exploitable live upload endpoint. **Remediation direction:** either delete the unused feature or implement safe pre-write directory/file handling and atomic creation, including ancestor symlinks and collisions. Test outside bytes remain unchanged. The download helper also buffers the full response before staging's size check (`file-handler.ts:171`); this is a related unbounded-I/O concern only if the feature is activated.

### 9. Reduce Commander's implicit rendering interface

Fallow 3.15.0 reports `render` at `hub-web/src/main.ts:1626` with cyclomatic complexity **131** and cognitive complexity **121**; `renderSessionState` at line 1018 has **77 / 111**. These are tool-specific static metrics, not performance measurements. `render` computes send policy, lease/control state, session navigation, attention, pairing and markup from module state (`main.ts:1648`, `:1680`, `:1685`).

The code already has meaningful seams: `renderDecision` can avoid rebuilding the shell (`main.ts:1746–1751`), `renderRegions` updates current regions (`:1863`), and terminal modules own their own behavior. This falsifies “every heartbeat rebuilds the whole UI.” **Remediation direction:** make each region consume a bounded view model, keeping side effects out of model construction. Test the observable DOM and transitions through those interfaces. Do not add a framework or split functions solely to reduce Fallow's number. Deleting the renderer would push real behavior elsewhere; the target is a smaller interface, not fewer files.

### 10. Share transaction lifecycle, not transaction semantics

Fallow found a 16-line clone between `hub-web/src/storage.ts:28–43` and `:212–227`. Source review confirms duplicated settlement, abort listener, error handling and database closure. Both paths open a database and close it independently. The general helper handles a single request; the machine backend performs an atomic read-modify-write (`storage.ts:228–233`).

**Remediation direction:** share private transaction-lifecycle handling, allowing the operation body to issue the required requests. Replacing the backend update with two general `transact` calls would break atomicity. Test real IndexedDB behavior or a faithful local implementation at the catalog interface, including cancellation before open, cancellation during work, request failure and successful commit. The deletion test favors one internal lifecycle owner; it does not justify exposing database requests to catalog callers. No current connection leak is claimed.

### 11. State the installer's artifact trust explicitly

`download_and_install` downloads over HTTPS, extracts, installs and on macOS removes quarantine (`scripts/cas-install.sh:211–259`). The subsequent `verify_install` begins at line 447; the inspected installer has no checksum/signature validation step. The evidence is the complete function and source search, not a network attack or release incident.

**Remediation direction:** verify an artifact receipt before extraction/replacement and preserve the prior executable until verification succeeds. A checksum fetched from the same compromised publishing authority detects corruption but does not independently authenticate that authority; specify the trust root for a signature/attestation if substitution resistance is the goal. HTTPS and an explicitly selected version are existing protections, so this finding is narrower than “unauthenticated download.” Whether the present trust model is acceptable is an operator decision; quarantine removal increases the value of making it explicit.

## Checked and rejected candidates

| Candidate | Evidence checked | Why it was rejected or narrowed |
| --- | --- | --- |
| “Store wrappers are useless pass-through modules.” | `cas-cli/src/store/quarantine_task.rs:3–19`, `:103–120`; `store/detect.rs:379–400`. | Quarantine filtering protects multiple list callers while keeping ID lookup inspectable. Deletion spreads real policy; forwarding methods alone do not make the module shallow. |
| “Disabled task notifications still fire in production.” | `cas-cli/src/store/notifying_task.rs:34–64`; `store/detect.rs:394`. | Transition branches omit an internal enabled check, but production wrapper construction requires notifications enabled. No production bug established. |
| “Fallow proves large amounts of dead code.” | Both raw JSON reports; `hub-web/src/connection-state-view.ts:37`, `connection-state.ts:62`; tests and import search. | The duplicate `elapsedSeconds` exports accept different snapshot shapes and deliberately delegate. Test-only exports, interface members and internal uses require case-by-case review. No bulk deletion list is endorsed. |
| “Fallow measured low production test coverage.” | Both health summaries report `coverage_model: static_estimated`. | No runtime coverage was supplied. CRAP/severity labels were not treated as measured coverage or incident severity. |
| “Checked-in web assets are avoidable duplication.” | `cas-cli/src/hub/server.rs:157–199`; `.github/workflows/ci.yml:362–370`. | Rust embeds dist assets; CI rebuilds and rejects drift. They are required offline build inputs. |
| “Shared live Cargo target would solve worker build cost.” | `CLAUDE.md:60–89`; `scripts/refresh-worker-build-cache.sh:35–46`. | The repo deliberately publishes a quiescent snapshot and uses private targets. A shared live Cargo target reintroduces serialization; this audit did not remeasure the documented sccache result. |
| “Release panic settings are reckless overhead.” | `Cargo.toml:37–56`; `cas-cli/src/lib.rs` panic guard; `CLAUDE.md:94`. | Unwinding is required by the MCP panic catcher. No abort optimization is recommended. |
| “Reference-history scanning should use only current content.” | `scripts/gen-builtin-reference-history.sh:4–10`, `:34–48`; `generator-inventory.json`. | The history is needed to recognize previously shipped files. There are 141 reference paths / 47 canonical groups; 28 groups are byte-identical across all three current flavors. Repeated Git/hash subprocesses are a candidate for batching, but historical blobs can differ and no elapsed-time hotspot was measured. Deferred below ranked work. |
| “Workspace crates form a cyclic dependency graph.” | Root and member Cargo manifests, recorded `workspace-dependencies.json`. | No cycle among the inspected workspace path dependencies. The evidenced boundary problems are inside application composition and the bridge execution path; a clean crate DAG does not prove clean internal architecture. |
| “All correctness defects are production vulnerabilities.” | Router allowlists, normal CI base guard, helper import searches, fixture-only outputs. | Staging is dormant, CI's invalid-base case is guarded, and no production data or external service was exercised. |

## What would falsify this verdict

The central verdict would weaken if integration evidence showed sync intent survives queue failures, generated frontmatter round-trips representative strings, and the active bridge recovers scoped identity and serializes requests. A close API that already exposes all effects and invariants independently of transport would overturn the proposed architectural priority. Those outcomes require execution at the identified boundaries, not a low aggregate linter score.

For each row, the ranked table states the more local falsifier. Evidence of real production frequency, operational cost or existing recovery paths could change priorities without changing the reproduced code facts. This audit establishes no regression start date, throughput loss, exploitable remote boundary, or exhaustive unused-code inventory.

## Prioritized next actions

Owners below are suggested component responsibilities, not assigned people or created tasks.

| Order | Suggested owner | Concrete next action | Acceptance evidence |
| --- | --- | --- | --- |
| 1 | Store/cloud maintainer | Design durable mutation/outbox semantics for finding 1. | Real SQLite fault and restart tests; local success is distinguishable from degraded sync. |
| 2 | Managed-artifact maintainer | Share YAML scalar serialization for finding 2. | Both active writers round-trip hostile and ordinary scalar inputs through their readers. |
| 3 | Bridge maintainer | Repair scoped session identity and choose the supported execution policy, findings 3–4. | No live Slack needed: fake process runner plus socket tests for concurrency, failure and restart. |
| 4 | Build/CI maintainer | Resolve Git paths and propagate classifier failure, findings 5 and 7. | Two unchanged builds stay fresh; changed refs refresh metadata; injected Git errors run the conservative tier. |
| 5 | Lifecycle maintainer | Extract one close service increment at a time, finding 6. | Existing gate contracts plus temporary Git/SQLite tests for success, refusal, reverse state and retry. |
| 6 | Bridge maintainer | Delete or repair dormant staging before activation, finding 8. | Outside bytes unchanged; symlink ancestors, collisions and bounded downloads covered. |
| 7 | Commander maintainer | Introduce bounded region models, then share transaction lifecycle, findings 9–10. | Observable DOM transitions and real transaction atomicity/abort tests; no metric-only acceptance. |
| 8 | Release maintainer | Decide and encode artifact trust for finding 11. | Invalid artifact rejected before extraction or replacing the installed binary. |

## Provenance and method

**Source of truth:** `docs/reports/2026-09-04-cas-src-codebase-audit.md`; the adjacent HTML is generated from this Markdown. **Repository snapshot:** `bbc1629b85ba891e248f6b328d9b78a9a45beaba`. Analysis date is 2026-09-04 UTC; evidence is a point-in-time source audit and synthetic local probes, not an operational telemetry window. No production database, credential file, cloud API or Slack account was read or changed.

The tracked inventory contains **4,019 paths**. Excluding `vendor/` and `.cas/`, extension counts include **1,219 `.rs`, 84 `.ts`, 82 `.sh`, 12 `.yml`, 3 `.yaml`, and 719 `.md` files**. Paths are counted, not executable modules; extension counts include tests and documentation. Vendor source, generated bundles, historical reports and binaries were inventoried but not subjected to a claim of complete manual review. Risk-based reading focused on active stores, lifecycle orchestration, build/CI scripts, managed artifact writers, Commander rendering/storage, and bridge routing/execution/staging.

| Fallow package | Dead-code/API candidates | Clone groups / duplicated lines | Complexity findings | Analysis time |
| --- | --- | --- | --- | --- |
| hub-web | 60: 31 exports, 2 types, 26 class members, 1 duplicate export | 6 / 108 | 51 | 3692 ms |
| slack-bridge | 10: 6 exports, 4 class members | 2 / 46 | 7 | 3511 ms |

Source: `fallow-hub-web.json` and `fallow-slack-bridge.json`, Fallow **3.15.0**, schema **10**, baseline above, default analysis without runtime coverage or custom boundary rules. The elapsed times are Fallow's own elapsed counters, not a benchmark. Zero reported boundary violations means no configured violations were found, not that architectural boundaries were proven sound. Fallow analyzed different file sets for clones and complexity: hub-web 44 clone-source files / 69 health files; slack-bridge 11 / 17. Test inclusion and entrypoint inference explain why these are not inventory totals.

**Tools:** Node `v24.19.0`; rustc `1.95.0 (59807616e 2026-04-14)`; cargo `1.95.0 (f2d3ce0bd 2026-03-21)`; Pandoc `3.7.0.2`. Fallow was run only in the two actual JS/TS packages. Shell/Rust findings came from code inspection and bounded probes, not Fallow. No product build, full Rust suite or production benchmark was run for this report-only change.

### Commands and durable evidence

Durable supporting receipts are under `/home/pippenz/.cas/artifacts/cas-1939/`. They are supplemental: all conclusions, qualifications and important probe outputs are included in this report, which remains readable offline without those files. Probe source and build scratch live under `target/cas-1939-probes/` in the audit worktree. `probe-receipts.json` records commands, exit codes and outputs; `source-evidence.txt` preserves numbered source slices.

The following inventory and discovery commands were run, followed by numbered reads of the exact cited files:

```bash
git status --short
git rev-parse HEAD origin/main
git log -1 --format='%H %cI %s'
git ls-files
rg --files -g 'package.json' -g '*.[jt]s' -g '*.tsx' -g '*.jsx' -g 'Cargo.toml'
rg -n 'SkillSyncer' cas-cli/src --glob '!skills.rs'
rg -n 'stageFile|downloadSlackFile|injectMessage' slack-bridge/src
rg -n 'ensureSubscription|threadRouting.set|activeSubscriptions.set' slack-bridge/src/daemon-main.ts
rg -n 'fn escape_yaml' crates/cas-core/src cas-cli/src
rg -n 'sha256|checksum|verify' scripts/cas-install.sh
rg -n 'classify-ci-diff|rev-parse|empty|web-check-needed' .github/actions/*/action.yml
```

Fallow commands, run from each named package directory (stdout preserved; `|| true` follows the installed skill's convention):

```bash
# cwd: hub-web
fallow --format json --quiet --explain > /home/pippenz/.cas/artifacts/cas-1939/fallow-hub-web.json 2>/dev/null || true
# cwd: slack-bridge
fallow --format json --quiet --explain > /home/pippenz/.cas/artifacts/cas-1939/fallow-slack-bridge.json 2>/dev/null || true
```

Both files parsed successfully as non-error combined schema-10 results. The shell command masks Fallow's underlying exit code, so no claim is made about that exit code. Summary counts were extracted from `check`, `dupes.stats`, and `health.summary`, with selected findings compared to source and imports; no automatic fixes or suppressions were applied.

The reproducible probe runner, `target/cas-1939-probes/run-probes.py`, executes the following operations and writes receipts. A durable copy accompanies the receipts for reviewers; it reads the audited source and confines fixture mutations to the task's probe directories.

```bash
python3 target/cas-1939-probes/run-probes.py
# classifier-invalid-ref.json: helper exit 0, stdout empty, fatal Git error
# thread-session-proof.json: two distinct inputs, same session ID
# staging-proof.json: ok:false, outsideContent:OVERWRITTEN, symlinkRemains:true
# yaml-proof.json: actual extracted Rust helper emits invalid YAML
# build-probe-second.log: unchanged fixture dirty because ../.git/HEAD is missing
```

The YAML probe compiles the function extracted from `cas-cli/src/sync/skills.rs` using `rustc`, then parses its emitted frontmatter with PyYAML. The thread probe extracts the actual function from `daemon.ts` and strips TypeScript types with Node's built-in API; it does not import or run a harness. The staging probe imports the actual `file-handler.ts`. The Cargo fixture contains the exact two `rerun-if-changed` directives and an analogous `.git` file; it is deliberately not the full cas build. The runner's assertions treat these observed defective outcomes as successful reproductions, not successful product behavior.

### Report validation

The final HTML is **56072 bytes**, below the 500 KB limit. It contains inline CSS, system fonts, no scripts and no external runtime resources. Source fidelity is checked by comparing normalized text from a fresh Pandoc render with the HTML main content after removing presentation-only table captions. Heading order, table captions/header scope, source hash and the two-file diff are checked by `validate-report.py`.

**Browser proof:** Google Chrome `152.0.7977.82`, headless, opened the local file with offline browser contexts. Desktop 1280 px, mobile 360 px and dark-theme desktop 1280 px each had document scroll width equal to viewport width, zero HTTP(S) requests, and identical main-content text. Mobile and dark contexts disabled JavaScript before navigation. Wide evidence tables scroll inside named keyboard-focusable regions; the page itself does not scroll sideways. Keyboard Tab reaches the skip link, and visible focus styles are defined. Static palette checks cover body/link/caption contrast on all report backgrounds in both themes. These are bounded accessibility checks, not a screen-reader audit or a claim of formal WCAG conformance.

**Print proof:** A4 print-media PDF, **13 pages**, with no overflowing table container; text extraction checks the major sections and PDF text bounding boxes against page bounds. Desktop, mobile and first/middle/last print-page images were visually inspected for legibility and clipping. Print forces a light theme and repeats table headings. The generated PDF is a validation artifact; the accessible HTML is the deliverable.

The connector's headed launch failed before navigation with `Error: server: Failed to launch the browser process.` and `Missing X server or $DISPLAY`; its guidance was `Looks like you launched a headed browser without having a XServer running.` The receipt is `headed-browser-error.txt`. No display server was installed or restarted. Local headless Playwright supplied the checks instead. The supervisor subsequently reported successful opening in the operator's interactive Chrome at 22:46:35 UTC: exit 0, `Opening in existing browser session.` This is a supervisor-supplied launch receipt, not an independent visual check of that window. The earlier failure is specific to the connector's display configuration; an operator desktop exists.

```bash
python3 target/cas-1939-probes/render-report.py
python3 target/cas-1939-probes/validate-report.py
node target/cas-1939-probes/browser-check.cjs
pdfinfo /home/pippenz/.cas/artifacts/cas-1939/report-print.pdf
pdftotext -layout /home/pippenz/.cas/artifacts/cas-1939/report-print.pdf /home/pippenz/.cas/artifacts/cas-1939/print-text.txt
```

Receipts: `report-validation.json`, `browser-proof.json`, `report-print.pdf`, `print-text.txt`, `desktop.png`, `mobile.png`, `dark.png`, and `print-page-1.png` / `print-page-middle.png` / `print-page-last.png` under the durable artifact directory. Browser and renderer scripts are preserved there as supporting evidence. The report can be reopened directly at `docs/reports/2026-09-04-cas-src-codebase-audit.html`; no server or build is required.

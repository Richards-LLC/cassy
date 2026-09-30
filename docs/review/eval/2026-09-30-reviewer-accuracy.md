# Keep reviews in shadow mode

The combined reviewers' valid reports caught **3 of 14 seeded defects (21.4%)**; legacy caught 4 (28.6%). Neither clears the proposed 90% recall gate. Fix safety remains unproven.

<figure class="hero-figure">
<svg viewBox="0 0 440 205" role="img" aria-labelledby="recall-title recall-desc">
<title id="recall-title">Usable reviewer recall is 21.4%, below the proposed 90% gate</title>
<desc id="recall-desc">The combined reviewers detected3 of 14 seeds in valid reports. Legacy detected4 of 14,28.6%, with source contamination. Proposed threshold90% remains unapproved.</desc>
<path class="axis" d="M155 38H415 M155 168H415"/>
<path class="proposal" d="M389 45V165"/>
<text x="389" y="24" text-anchor="middle">90% proposal</text>
<text x="0" y="76">Reviewers 3/14</text><path class="axis" d="M155 71H211"/><circle class="decisive" cx="211" cy="71" r="6"/><text x="225" y="76">21.4%</text>
<text x="0" y="128">Legacy 4/14*</text><path class="axis" d="M155 123H229"/><circle class="measured" cx="229" cy="123" r="5"/><text x="242" y="128">28.6%</text>
<text x="155" y="192" text-anchor="middle">0%</text><text x="285" y="192" text-anchor="middle">50%</text><text x="410" y="192" text-anchor="middle">100%</text>
</svg>
<figcaption>Protocol-valid recall, 14 seeded instances; extraction 2026-09-30 UTC, reviewer-eval.py score and metrics.json. *Legacy includes four contexts that read live skills outside the replay; these cannot establish source-isolated accuracy.</figcaption>
</figure>

| Shared population | Valid-report seed hits | Recall | Proposed minimum |
| --- | ---: | ---: | ---: |
| Combined reviewers | 3/14 | 21.4% | 90.0%, unapproved |
| Legacy verifier | 4/14 | 28.6% | 90.0%, unapproved |

## Accuracy and what the numbers mean

| Actor | Seed recall | Class recall | Precision on resolved findings | Precision bounds | False-flagged clean cases | Invalid reports |
| --- | --- | --- | --- | --- | --- | --- |
| spec | 3/11 (27.3%) | 2/9 (22.2%) | 15/25 (60.0%) | 40.5%–73.0% | 3/4 | 2/20 |
| standards | 0/3 (0.0%) | 0/3 (0.0%) | 15/15 (100.0%) | 71.4%–100.0% | 0/4 | 1/20 |
| baseline | 4/14 (28.6%) | 2/12 (16.7%) | 27/33 (81.8%) | 73.0%–83.8% | 3/4 | 0/20 |


Spec caught 3/11 assigned seeds, Standards 0/3. Standards found three Spec seed instances through its test-confidence lens; the c05 report was invalid, so it supplies content evidence but no usable coverage. Raw combined content recall is 4/14 (28.6%); valid combined recall is 3/14. Both collapse to2/12 independent defect classes (16.7%). The two-axis content result ties legacy recall; valid coverage trails legacy by 7.1 percentage points. Standards'100% resolved precision is conditional on only 15 resolved findings, with six unresolved judgments and zero assigned seeds caught; it is not a safety endorsement.

Precision counts serialized finding occurrences, including scope-creep entries and repeated IDs in invalid reports. Spec has 15 true,10 false and 12 unresolved occurrences; Standards 15 true,0 false and 6 unresolved; legacy 27 true,6 false and 4 unresolved. All93 distinct keys have explicit independent labels; duplicates yield95 occurrences. Bounds assign unresolved findings all false/all true. They are adjudication bounds, not statistical confidence intervals. Gold seed matches are explicitly recorded in adjudication.json; there is no keyword oracle. Unexpected real findings count toward precision, without inventing seeded recall. Test-confidence judgments are labeled in the raw reports and held unresolved where seam coverage or policy cannot be established.

Spec and legacy each falsely flag 3/4 eligible clean cases. Those failures include reversing approved --lib, default-branch PR and metadata-normalization policies because current stored original task criteria omitted later operator clarifications. This measures the combined context-and-review system, not a model's intrinsic accuracy. Clarify canonical requirements before another gate run.

## Corpus and paired harness

Twenty pinned authentic historical slices cover the 3.37 and 3.38 release cycles:14 defects across 12 classes,4 eligible corrected-delivery negatives (c15,c16,c17,c20), and 2 nominal negatives retained as contaminated (c18,c19). c04–c06 share one self-reexec regression. Corrective commits, exact base/head SHAs and scoped files are in corpus.json/repairs.json. These are curated real fix cases, not a random holdout; six seeds are genuine fixture failures, rather than claims of corresponding production failures.

Two independent discoveries were preserved: c18 drops restored hooks from CI impact inventory (cas-045f), and c19 accepts a log redirection that creates an untracked file before clean_head refuses the check (cas-c0ec). Both are excluded from the clean denominator. Supplemental c20 reuses the authentic corrected factory child-test file at 0a93e428c after c19 contamination. Its narrow scope and post-discovery selection add selection bias; it is not a new independent delivery. All three actors emitted zero findings on c20.

Each actor received a fresh shallow replay checkout with the authentic head and parent, no origin remote or future repair object. Spec received exact stored original task text/criteria; Standards received only the real pinned Standards snapshot and proven rules 026/172/175; legacy received the pinned task-verifier body and the same task/standards inputs. The CAS task text is a current read-only DB export, not an immutable historical task snapshot. The canonical source pin is 175f78c55d32c20a1b67b1b61901a30004175973, including root CODING_STANDARDS.md. Sources were not invented to make seeds detectable. In particular, the Standards sources do not explicitly mandate the shell-interpolation or operator-organization hygiene seeds, making0/3 assigned recall partly a source-coverage limit.

All actors used the same configured gpt-6-sol model with high reasoning, independently logged usage and prompts; per-run emitted model IDs/configuration are in results.json. No Rust build or test was run. Python/static checks inside the model traces do not substitute for Rust execution. Fixers remained workspace-write sandboxed; cross-checks and legacy remained read-only. The runner's approved commit bridge mechanically committed only reviewer-selected files within each replay's exact scope, recording every request/response/SHA. The boundary suite passed before each batch; final suite 11/11 verifies ownership, metadata/outside-scope rejection, hidden-future exclusion and error denominators.

The API-authority and report-sealing protocol were **simulated**. Production shadow protocol uses side refs directly. All26 bridged commits below are part of the measured simulation, not registered production reviewer receipts.

## The source boundary was not fully isolated

Live skills were successfully read outside the replay in baseline c01,c04,c06,c20 and Spec c16. source-boundary-audit.json records the exact commands. These five contexts are quarantined for any claim of source-isolated accuracy; workspace-write restricted writes but did not prevent external reads. The observed numbers retain those runs transparently. The baseline's strictly uncontaminated contexts detected only c03/c05; excluding contaminated contexts as coverage misses gives2/14, not a clean paired estimate. No hidden curator corpus/repair/adjudication read was observed in the inspected command traces, but a lexical trace audit is not a general sandbox proof. No observed Rust invocation or nested reviewer delegation was found in the inspected commands; commands reading/searching Rust tool text were distinguished from executing tools.

## Recall by real defect

| Case | Expected axis | Named defect | Spec hit | Standards hit | Legacy hit |
| --- | --- | --- | --- | --- | --- |
| c01 | spec | Assembly receipt fingerprints workspace member version values and generated reference history; prep and ledger change those bytes and defeat intended reuse. | No | No | No |
| c02 | spec | Scoped Validation condition removes the default-branch PR exclusion, expanding the operator-approved admission-only policy. | No | No | No |
| c03 | spec | Inventory sees root tests/*.rs only; directory-main hook suite is omitted from the grouped harness, dropping 34 executed tests. | Yes | Yes | Yes |
| c04 | spec | After grouping, the isolated self-reexec still uses an unqualified libtest name and may execute zero tests while returning success. | Yes | Yes | Yes |
| c05 | spec | After grouping, the isolated self-reexec still uses an unqualified libtest name and may execute zero tests while returning success. | No | Content only; invalid | Yes |
| c06 | spec | After grouping, the isolated self-reexec still uses an unqualified libtest name and may execute zero tests while returning success. | Yes | Yes | Yes |
| c07 | standards | Full-suite shard inserts a GitHub expression directly into the shell run block for --base-sha. | No | No | No |
| c08 | spec | Recovery fixture supplies caller updated_at as the optimistic-lock token, although TaskStore::update owns and returns the stored timestamp. | No | No | No |
| c09 | spec | Closed fixture without dispatch expects the proof-scope helper to reject terminal status changes, although terminal policy belongs to the update handler. | No | No | No |
| c10 | spec | Fixture compares canonical sorted/deduplicated scoped targets to traversal order, rather than exact set and uniqueness. | No | No | No |
| c11 | standards | Allowed-check rewrite fixture redirects logs to bare /tmp and inherits an unrelated clone binding; recursive workspace enforcement correctly denies it. | No | No | No |
| c12 | spec | Abandoned relay fixture unwraps a transition to Delivered, although both are rank-3 terminal siblings and the transition is rejected. | No | No | No |
| c13 | spec | Builtin workflow fixture still requires the old assembly-command phrase after the canonical workflow switches to the two-context assembly-proof command. | No | No | No |
| c14 | standards | Shipped reusable registration instructions contain a concrete operator organization/tracker issue. | No | No | No |


The captured seeded hits cluster in dropped hooks and grouped self-reexec. No actor caught the named metadata normalization, default-branch policy expansion, optimistic-lock fixture, Closed helper fixture, canonical target-order fixture, bare-/tmp fixture, terminal-sibling fixture, stale builtin phrase, interpolation or organization-hygiene seed. Unseeded findings include broken Nextest failure parsing, stale CI binary selection and Violet setup/runtime mismatch; these do not compensate for missed seeds.

## Commit safety and transport failures

Other-axis cross-checks accepted19/26 actual fix commits, requested **zero reverts**, and left7 pending behind invalid independent pairs. Independently verified targeted tests are unknown for all 26. The observed bad-commit rate is therefore **0/26, a lower bound with possible range0–100%**, under the requested definition (other-axis revert or targeted-test failure). Legacy has no fix denominator: N/A. This does not demonstrate zero bad commits.

The curator found gold-policy reversals despite acceptance: c11 Spec removes authorized --lib behavior, c15 Spec reintroduces issue5 into configured tracker guidance, and c17 Spec's two commits remove approved version/ledger normalization. The c17 pair remains invalid and was not cross-checked; c11 and c15 were accepted by Standards. These are separate policy correctness concerns, not fabricated failed-test receipts. c01's removal of documented24h freshness remains a policy dispute. Cross-axis agreement alone did not establish correctness.

Three model protocol failures remain intact: Spec c09 cites nonexact finding sources (and repeats one commit across entries), Spec c17 repeats finding IDs, and Standards c05 declares only the corrective one of its two actual commits. Invalid runs remain in coverage/error denominators; their raw findings remain in precision and all actual commits remain in safety denominators.

Three earlier c08/c09/c10 Spec attempts failed before inference with provider HTTP400 invalid_json_schema because a strict enum contained a quoted criterion. Original directories are archived under pre-inference-schema-failures; no tokens or fixes were generated. Corrected schemas preserved exact criterion text and successful retries are the primary runs. A baseline c15 adapter validation mistake applied Spec-only status constraints to legacy VERIFIED; its original result is retained, corrected locally without changing its raw report or rerunning the model. Scheduling clone-ownership conflicts between two batch runners produced no duplicate model contexts or overwritten reports. Pilot and startup failures are excluded from primary accuracy/cost figures and retained as transport evidence.

| Case / axis | Actual fix commit through bridge | Other-axis decision | Targeted tests |
| --- | --- | --- | --- |
| c01 / spec | `96fdb3948b6ec6d6afdb8f33ed0abfc98b016719` | accept | Unknown |
| c01 / standards | `33b236ff4d6cf45e2b649f65c91f64d2d392c708` | accept | Unknown |
| c02 / spec | `d08cf22646ba8733af3083dd277174866786b878` | accept | Unknown |
| c02 / standards | `dd5de4de656afcfc1f4999fc12c50de8fc3d9757` | accept | Unknown |
| c03 / spec | `ea3668ddd692283755e954ab2f0acbbbd8fba136` | accept | Unknown |
| c03 / standards | `d1f53643ea8cff717aaa91f1077e29ec9185050e` | accept | Unknown |
| c03 / standards | `116c6290b15eb196028ae1f9b375ec916ad725ff` | accept | Unknown |
| c04 / spec | `c202a413c218f77406b207c52716eb0f499e333c` | accept | Unknown |
| c04 / standards | `5e6fcdebcb1849e8023d815c1bdb75bcbf94bcff` | accept | Unknown |
| c05 / standards | `093b1a8e34c382eb25d58e888122229c159aab6c` | Pending; invalid pair | Unknown |
| c05 / standards | `6e985b0d8ec0ed253ca05875db7ecbc1acdbc7e4` | Pending; invalid pair | Unknown |
| c06 / spec | `4cd92e9cc7984117905c3f4927d1af1d8ab2c69e` | accept | Unknown |
| c06 / standards | `d9db0de12c1a1c656d1775a7ce1eeaccc608e8a2` | accept | Unknown |
| c09 / spec | `83b9e3b303ace1bf3277885b5cb840bfc11d8375` | Pending; invalid pair | Unknown |
| c09 / spec | `77ee4caa50e0adcc0950658cb867a964c5aa456b` | Pending; invalid pair | Unknown |
| c09 / standards | `ef443e654a97370177a400a0e0fd0c4f53b660f5` | Pending; invalid pair | Unknown |
| c10 / standards | `1e394a71bde1cbdbe60d87f6417b931688239abb` | accept | Unknown |
| c11 / spec | `1b996bb10468c6ad39f1f1e1f6585929d009a383` | accept | Unknown |
| c12 / standards | `eaf81b355cfc7131dabe5a7e75f06aa8843d16f2` | accept | Unknown |
| c13 / spec | `12f6e316bed1c88e2b38f90da2dc19830d46ba29` | accept | Unknown |
| c14 / spec | `fa164a462d4971671c218c99e9d096e94f84ed6c` | accept | Unknown |
| c15 / spec | `189e4724c46f5fb76204317ea346e1377edf537d` | accept | Unknown |
| c17 / spec | `634d0d4d25896b8c8370482ba57066202e0f8de4` | Pending; invalid pair | Unknown |
| c17 / spec | `686e5782ec58f0d86fb0edee26ebacce86055400` | Pending; invalid pair | Unknown |
| c18 / spec | `1b5660f89b189668b9e01a20fad487abb4a974bb` | accept | Unknown |
| c19 / standards | `674ce9c1e034c81a32dc42b7f5be6e39a35a1d9b` | accept | Unknown |


Exact selected files, bridge responses, commit subjects and full SHAs are in bridge-commits.json. Raw isolated repositories retain each commit for supervised targeted testing. Unknown is deliberate; test receipts must not be inferred from reviewer approval.

## Tokens and time

| Actor | Input tokens (includes cached) | Cached input | Output tokens | Summed actor seconds | Cross-check input / cached / output | Cross-check seconds |
| --- | ---: | ---: | ---: | ---: | --- | ---: |
| spec | 20621379 | 18939392 | 156660 | 4078.9 | 933674 / 773504 / 10933 | 351.8 |
| standards | 13082332 | 11853312 | 111062 | 2932.8 | 1176482 / 942848 / 15106 | 425.6 |
| baseline | 13720721 | 12187904 | 150018 | 4047.1 | 0 / 0 / 0 | 0.0 |


Input counts include cached tokens; do not add the cached column a second time. These are emitted cumulative multi-turn usage values, not one prompt's context length or priced cost. There are no missing primary token receipts. Independent times are summed actor wall time, not sequential batch latency. The observed concurrent batch envelope is 5391.4s (89.9min), from2026-09-30T02:43:08.988649+00:00 to2026-09-30T04:13:00.366109+00:00; it includes staged scheduling, approval/analysis gaps and supplemental runs, so it does not rank actor latency. Cross-check time/tokens are additional measured model work, separately reported.

## Proposed thresholds and the operator's decision

Proposals were recorded before full grading in protocol.json; **operator_approved=false**, **merge_policy_changed=false**. Require assigned-axis, combined and class recall≥90%; adjudicated precision≥95%; zero clean-negative false findings; zero bad commits; zero protocol/process errors; zero unresolved findings; and complete cross-check plus targeted-test receipts for every actual fix. Combined reviewers must meet or exceed legacy recall and precision on identical corrected inputs. This batch fails the proposal; do not relax thresholds to fit it. Small correlated samples cannot establish a future zero bad-commit probability; expand the independently held-out set before enforcement.

Keep both merge-policy adoption and two-way-door auto-merge held. The operator approves thresholds after canonical criteria/Standards coverage is clarified. A **successful rerun through the real registered shadow protocol after 3.39.0 is installed is a stated precondition before the operator enables any merge policy**. That rerun must exclude live external skill reads, preserve dispatch/round/identity/verification/cross-check receipts and obtain supervisor targeted-test/assembly proof. The committed registered adapter binds existing real actors; it does not synthesize identities or claim this simulation is registered authority. No merge policy has changed.

## Provenance and rerun

Analysis checkout checkpoint:a86ec77d6fb09fcfb92b8071938a27da1eb9a176; reviewer source:175f78c55d32c20a1b67b1b61901a30004175973. Data window:2026-09-30 UTC above. Raw data:/home/pippenz/.cas/artifacts/cas-b622/measured; pilot/failed transport logs remain alongside it. Durable copies:results.json,metrics.json,adjudication.json,bridge-commits.json,source-boundary-audit.json. No operational secrets are included.

```bash
REVIEW_EVAL_TEST_DIR=/home/pippenz/.cas/artifacts/cas-b622/python-test python3 scripts/test-reviewer-eval.py -v
python3 scripts/reviewer-eval.py run --out /home/pippenz/.cas/artifacts/cas-b622/measured --rules docs/review/eval/promoted-rules.json --reviewer-sha 175f78c5 --jobs 3 --fix-transport bridge
python3 scripts/reviewer-eval.py cross-check --out /home/pippenz/.cas/artifacts/cas-b622/measured --rules docs/review/eval/promoted-rules.json --reviewer-sha 175f78c5 --jobs 3
python3 scripts/reviewer-eval.py score --out /home/pippenz/.cas/artifacts/cas-b622/measured --labels docs/review/eval/adjudication.json
python3 scripts/report-reviewer-eval.py --out /home/pippenz/.cas/artifacts/cas-b622/measured
```

Use a fresh output directory for an intentional rerun; completed runs are reused and source-pin drift is rejected. Re-adjudicate new findings independently, then regenerate this Markdown and its HTML together. Production rerun uses --registered-adapter scripts/reviewer-eval-registered.py with supervisor-provided registered bindings (README.md); real receipts are required. Rendering reads this Markdown as its sole content source and uses inline house tokens; HTML is network-free and readable with JavaScript disabled.

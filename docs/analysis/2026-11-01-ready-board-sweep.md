# Ready-board sweep — November 1, 2026

Portable evidence: `art-65cc5389` (report, all source checks, CSV and executable coverage/ancestry validator).

125 tasks reviewed: **12 shipped, 8 obsolete, 105 retained**. The startup ready list had 122 tasks; three newly ready tasks were added to the cutoff ledger. The historical title's 126 was not the count at startup.

Was: 122 ready tasks and 18 open epics. Now: 118 ready tasks and 15 open epics at the 4:48 PM ET checkpoint. Four baseline rows are terminal: the 3.88, 3.89 and chat-rendering epics closed, and the obsolete contract QA child cancelled. The supervisor made those administrative changes. Three original rows entered active work and three new rows appeared: `122 + 3 new − 4 terminal − 3 active = 118`. Source fixes were neither staffed nor started by this sweep. Later task changes are outside this frozen comparison.

## Scope and evidence

- Source truth: `origin/main` at `4d893d0c53bd333b3640dda0836ce2bdfcea4e8e`, tag `v3.95.0`.
- One ledger row for every startup-ready task plus all three additions visible at the cutoff. Disposition is separate from current database status: **shipped** does not assert that a close gate accepted the administrative close.
- Each live finding is either a source inspection, a missing acceptance-proof determination, or a preserved owner/operator hold. Uncertainty stays live. Exact bounded queries and excerpts are in `source-checks.md`; existing executed tests are explicitly attributed to their original receipt. This no-code sweep ran no Rust builds or browser journeys.
- Child census uses all statuses, never `status=open`. MCP child lists were inspected without a status filter. A supplemental read-only snapshot includes quarantine-hidden rows; those are counted below. Zero children does not prove an unimplemented epic shipped.
- The seven dependency-security children are untouched. Operator/measurement holds and 3.96/prowl work are untouched. The split 3.97 board is not a close candidate. Accounts belongs to prowl, verification owner `ef2c9a21`, and is a recommendation only.
- Every unprotected retained startup implementation row and retained epic received a one-line sweep note. Held/protected rows remain unchanged. Active QA was not repeated or replaced with a fabricated verdict.

## Shipped work still needing administrative disposition

| Task | Shipped release | Commit and check | Existing executed evidence | Disposition |
| --- | --- | --- | --- | --- |
| cas-206a | v3.86.0 | `d1fed7313435b97b9910fa95d95735485104bce0`; ancestry checked against pinned main and owning source inspected | art-e4852184 | open; Six original failures and 178 module passes; release queue40646861659. Legacy ledger blocks ordinary close. |
| cas-b2d8 | v3.85.0 | `331f150a48c885ab9ee851a8a7938d23b8b9da4c`; ancestry checked against pinned main and owning source inspected | art-c469d6bf | open; Midnight timestamp defect fixed by 331f150a4 (v3.85); existing cas-9912 proof art-c469d6bf covers pinned midnight windows and affected modules. |
| cas-0ad2 | v3.86.0 | `98e63a29095a0af9f1a53730d412d30ad98602ac`; ancestry checked against pinned main and owning source inspected | art-d991581f | open; 98e63a290 (v3.86) clears all selectors before binding synthetic child homes; cas-cba9 and cas-5b47 receipts cover real path and hostile ambient selectors. |
| cas-d587 | v3.92.0 | `e534d3b20185d3e7d6b35a6bd5357b19b676cc24`; ancestry checked against pinned main and owning source inspected | qab-fbb74be095834b00bf43e02cce3d2230 | open; e534d3b20 and merge a86328e33 shipped v3.92; batch QA7 R26 HUB-J4 PASS and 51 vitests recorded on task; stranded anchor prevents admin close. |
| cas-1060 | v3.88.0 | `f02bfbf7565dc5a858c374742f90991ec9df166c`; ancestry checked against pinned main and owning source inspected | art-ef726eff | open; Shipped cas-2850/01657cbf3 (v3.88) syncs independent QA and evidence; corrective f02bfbf75 covers task-bound exports, remote paths and cache; art-ef726eff has 866 targeted passes. |
| cas-49e4 | v3.85.0 | `331f150a48c885ab9ee851a8a7938d23b8b9da4c`; ancestry checked against pinned main and owning source inspected | art-c469d6bf | open; Same midnight timestamp bug as cas-b2d8; 331f150a4/v3.85 fixes instant comparison and preserves old-history exclusion; existing art-c469d6bf is exact test evidence. |
| cas-be50c | v3.81.0 | `3a00059ef68d2150ad444313eef90b8d169acd93`; ancestry checked against pinned main and owning source inspected | art-6d086982 | open; 3a00059ef/fad204a3b (v3.81) default filtered task waits to TTL0 and emit explicit-expiry notice; real scanner regression and 612 affected tests in art-6d086982. |

The reminder rows describe the same repaired midnight history bug; keep one historical root if retaining a record is useful. Current source uses `julianday` rather than lexical timestamp comparison, pins both sides of midnight, and preserves exclusion of old history. The existing `cas-9912` receipt records 426 affected daemon/doctor/reminder tests, 24 reminder-store tests and 238 factory integration tests.

`cas-33f6` deliberately stays live: the current-source Zig resolver shipped, but `factory_worker_check.rs:3566` explicitly leaves discovery to the checked-out build script. That does not prove the stronger promise for older tips that lack the resolver. An earlier candidate recommendation was corrected to the supervisor before disposition.

## Exact administrative refusals

`cas-206a`, `no_delivery=true`:

```text
NO DELIVERY CLOSE REJECTED: recorded delivery, methodology or handoff evidence exists; use the ordinary close contract.
```

`cas-206a`, ordinary close with original `commit_receipt=d1fed7313435b97b9910fa95d95735485104bce0`:

```text
QA LEDGER REJECTED before verification dispatch:
line 1 [header]: canonical evidence matrix header is missing; run cas qa ledger init --task <id>
line 1 [rows]: expected at least 1 rows, observed 0; include the demo happy path and three independent conditions
line 1 [cells]: headline cells must equal 0; update the headline from the parsed row totals
line 1 [PASS]: headline PASS must equal 0; update the headline from the parsed row totals
line 1 [FAIL]: headline FAIL must equal 0; update the headline from the parsed row totals
line 1 [NOT EXERCISED]: headline NOT EXERCISED must equal 0; update the headline from the parsed row totals
line 1 [labels]: labels must equal {}; set labels: <label>=<count> for each used label
```

The guard named the historical `cas-206a/LEDGER.md`. Its bytes were not rewritten and its completed tests were not rerun.

`cas-0ad2`, `no_delivery=true`:

```text
NO DELIVERY CLOSE REJECTED: HEAD@cbe2ae3f64cc4e7d19262c1c8058208372f6fc5d has unmerged work or unreadable history
```

The worker checkout was independently clean at `ce03d1db75fe064848299d5216c27ba8b44f81ba`; the reported MCP HEAD was different. The rejection is recorded as an observation, not a proven routing cause.

`cas-706a`, cancel as superseded by `cas-4069`:

```text
TASK CANCEL REJECTED: only a live registered supervisor may cancel or reopen cancelled work. Caller `gentle-jaguar-71` has role worker.
```

The supervisor owns the remaining administrative dispositions. No status was forced, evidence rewritten, or new verification invented. `task action=list epic=cas-4744 offset=200 limit=200` also repeated the first 200 rows; the full census used the read-only dependency snapshot instead of mistaking that page for complete coverage.

## Obsolete rows

| Task | Reason and surviving owner |
| --- | --- |
| cas-706a | Existing supervisor decision identifies duplicate `cas-4069`; shipped `f0da391ed`, proof `art-4d702ad5`/`art-3785d18e`, 12,400 complete-file passes after the original coalescing control failure. |
| cas-9fe5 | `800ebb361` isolated the account-selector fixtures; `6253627f6` later retired the release-train script/tests. Its named fixture target no longer exists on main. |
| cas-68bd | Superseded by approved/shipped `cas-e0d4`, commit `8f28ee6bd`, issue-backed fail/pass handling and quarantine; real hosted proof `art-70fde6f9`. |
| cas-6c60 | Exact reminder `target=self` duplicate of older `cas-d398`; keep the older live bug and its real-dispatch acceptance. |
| cas-4004 | QA for closed/shipped `cas-f2ad`; already cancelled by supervisor reconciliation. |
| cas-21c4 | Old QA round for closed/shipped `cas-326e8`; supervisor `qa_reconcile` disposition, no new verdict. |
| cas-c053 | Old QA round for closed/shipped `cas-89b0`; supervisor `qa_reconcile` disposition, no new verdict. |
| cas-f25d | Old QA round for closed/shipped `cas-310f`; supervisor `qa_reconcile` disposition, no new verdict. |

`cas-c93a` is a new, active adversarial review for `cas-77872`, not an obsolete round. It stays live.

## Open-epic census and release mapping

Counts below include quarantine-hidden rows. The 3.88 list exposed 56 visible rows, while the full census/supervisor close used 57; the additional row was terminal. Counts are point-in-time and include administrative reconciliation during the sweep.

| Epic | Children | All terminal? | Release containing shipped work | Action |
| --- | ---: | --- | --- | --- |
| cas-f1f6 | 16 visible at first list;17 raw later | no | 3.96 not on pinned main | Prowl-owned active board; untouched. |
| cas-b053 | 80 | no | v3.86.0 core; later fixes v3.87+ | 50 closed,11 cancelled,19 open before cleanup; retain. |
| cas-e46e | 78 | yes | v3.83.0 core (5a234f9fc); fixture follow-up fd64b6c4f in v3.86.0 | Supervisor candidate; stranded branches/evidence still require review. |
| cas-8f96 | 8 at first list;12 raw later | no | 3.97 not on pinned main | Split active board; untouched. |
| cas-40c7 | 5 | no | phase1 enters3.96; other policy phases unshipped | 1 closed,4 open; retain full epic scope. |
| cas-4744 | 229 | no | v3.92.0 through v3.95.0; latest merge4d893d0 | 215 closed,4 cancelled,10 open before cleanup; retain. |
| cas-1d5c | 21 | no | v3.91.0 core (36f17f939); carries v3.92.0 | 19 closed,2 open; retain macOS/ref-resolution follow-ups. |
| cas-5de5 | 17 | no | guidance/queue recovery in v3.92–v3.95 | 10 closed,7 open at census; retain capability/runner/source-resurrection work. |
| cas-c10b | 6 after reconciliation | yes | contract/code frames v3.87.0; typed-row continuation v3.88.0 | Closed by supervisor after patch-equivalence evidence below; follow-ups preserved on live boards. |
| cas-b9d4 | 19 | no | v3.85.0 code (75afaf588) | 18 closed,1 open live-measurement task; retain. |
| cas-5b50 | 42 | yes | v3.84.0 core (941695ed7); late usage fix v3.88.0 (1229fbfe6) | Prowl owns closure; do not count as local supervisor work. |
| cas-1979 | 53 | no | v3.83.0 core; follow-ups through v3.87 | 46 closed,6 cancelled,1 blocked C10 downstream flow walk; retain. |
| cas-7fdd | 0 | no implementation proof | none | No Android app delivery established; empty child set is not completion. |
| cas-cde3 | 41 | no | v3.90.0 core (ffe6a40ab) | 2 AwaitingMerge,2 InProgress,1 open QA at census; retain. |
| cas-caf5 | 45 | yes | v3.89.0; main merge 4d48516c6 | Closed by supervisor; all45 terminal. |
| cas-9d08 | 57 | yes | v3.88.0; main merge b0bff674b | Closed by supervisor; all57 terminal. |
| cas-5cac | 48 | no | multiple releases v3.78+ | 15 closed,2 cancelled,26 open,5 blocked at census; retain live ownership/measurement holds. |
| cas-2907 | 7 | no | unshipped held security work | All seven children operator-held; untouched. |

## Stranded typed-work lane: already shipped, no merge

The supervisor asked whether two non-merge commits on `factory/kind-finch-39` were missing from main. They are already delivered by `cas-d278` through another route:

| Stranded commit | Main ancestor | Executed comparison |
| --- | --- | --- |
| e3995e828bf021ada8c2e4bd2dd8854c9805d336 | 3c25c3fb3 | Stable source patch ID, excluding generated `hub-web/dist`: both `fb8804915534d0b566405d3ab433448030038947`. |
| 3be87f246f56e5bc2759032ee944042b7da888a6 | fa8dc2009 | Whole patch ID: both `558ba86cf728d8fc529e89fb8441c6052cd9f394`. |

`git diff e3995e828 3c25c3fb3` on `work-events.ts`, `work-rows-view.ts`, `work-picture.test.ts` and `worker-timeline.ts` is empty. Both replacement commits are main ancestors and first appear in tag `v3.88.0`. Later refactors retain typed read/edit targets, search counts, command/cwd/exit metadata, exact raw records, failure labels, folded turn notices and shared worker rendering. Main references: `work-rows-view.ts:186,196,293`, `worker-timeline.ts:79`, `whole-picture.journey.ts:197,229`, and `work-picture.test.ts:346`.

This is static equivalence and shipped-bundle inspection, not a newly executed browser acceptance claim. No stranded content was merged.

## Ten live items by cost today

This ranking is an assessment of observed current cost, not a staffing instruction. Security and operator decisions remain held; prowl retains its board.

| Rank | Task | Current cost and next evidence needed |
| ---: | --- | --- |
| 1 | cas-e999 | A real operator message waited11m30s; fresh heartbeat denied six wakes. Needs exact-candidate enqueue-to-turn proof and truthful read/answered label. |
| 2 | cas-209f | Running sessions can disappear and a recycled name can receive an older held message. Needs identity-safe native reproductions; prowl owns it. |
| 3 | cas-b815 | A live supervisor can be reported absent, making finished workers report false blocks and spend another turn. Needs session/clone resolver readback; prowl owns it. |
| 4 | cas-0f55 | At least six redundant alerts during an already-owned verification/proof wait spend supervisor turns. Needs one alert until state/deadline changes. |
| 5 | cas-d622 | Agent-doc budget failures first surfaced in two release queues; first repair cycle cost about65min. Needs a PR-shaped fixture proving existing lane selection catches them earlier. |
| 6 | cas-7ef5 | Phone header clips Show work at390px; newer cas-a929 adds attention-badge reproduction. Needs real before/after with all controls visible. |
| 7 | cas-11c0 | Capped runner refuses a legitimate Cargo-discovered integration harness, leaving affected consumers unexecuted. Needs true-positive and unknown-harness negative admission controls. |
| 8 | cas-db56 | One park needs three close calls because close-owned notes can invalidate its snapshot. Needs the advisory+batch-deferral first-call case and real concurrent-update control. |
| 9 | cas-490b | Tested per-delivery Violet routing has no production writer, alongside three other wire-or-delete decisions. Needs real caller trace and one failing-first behavior test if wiring is intended. |
| 10 | cas-7b24 | QA trace resources can retain opaque WebSocket ticket query values. Needs count-only real trace scrub proof without printing values. |

## Full disposition ledger

Current status is the recorded status at report generation, not a completion claim. Each row's source details can be found under its ID in `source-checks.md`. Held rows cite their preserved hold instead of accessing live accounts or changing repository settings.

| Task | Priority | Outcome | Current status | Evidence and remaining work |
| --- | --- | --- | --- | --- |
| cas-209f | P0 | live | awaiting_merge | 3.96/prowl scope: live-session and identity findings remain assigned there; no changes. Source: `hub-web/src/connection.ts:2`. Hold: 3.96/prowl or split 3.97 scope. |
| cas-e999 | P0 | live | awaiting_merge | 3.96/prowl scope: operator fresh-heartbeat wake refusal remains reported; no changes. Source: `cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs:7117`. Hold: 3.96/prowl or split 3.97 scope. |
| cas-b815 | P0 | live | in_progress | 3.96/prowl scope: live registered supervisor lookup incident remains; no changes. Hold: 3.96/prowl or split 3.97 scope. |
| cas-f1f6 | P0 | live | open | Active protected release board; no closure requested. Hold: 3.96/prowl or split 3.97 scope. |
| cas-206a | P0 | shipped | open | v3.86.0; `d1fed7313435`; art-e4852184. Six original failures and 178 module passes; release queue40646861659. Legacy ledger blocks ordinary close. Source: `cas-cli/src/builtins/skills/cas-worker.md:2`. |
| cas-b053 | P0 | live | open | All-status child audit: 80 children, {'closed': 50, 'open': 19, 'cancelled': 11}; retain nonterminal or unimplemented scope. |
| cas-e46e | P0 | shipped | open | All-status child audit: 78 children, {'closed': 53, 'cancelled': 25}; terminal candidate; supervisor owns branch-content disposition. |
| cas-8f96 | P1 | live | open | Active protected release board; no closure requested. Hold: 3.96/prowl or split 3.97 scope. |
| cas-d622 | P1 | live | open | No executed PR-shaped agent-contract routing proof; retain recent two-ejection wiring defect. |
| cas-3b9f | P1 | live | open | Guidance ships; operator-approved audit capability, detector precision and downstream proof still required. |
| cas-dc36 | P1 | live | open | Structural parser redesign awaits operator decision; existing token scanner remains. Source: `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:3329`. Hold: operator/measurement/live-proof hold; unchanged. |
| cas-40c7 | P1 | live | open | All-status child audit: 5 children, {'open': 4, 'closed': 1}; retain nonterminal or unimplemented scope. |
| cas-4744 | P1 | live | open | All-status child audit: 229 children, {'closed': 215, 'open': 10, 'cancelled': 4}; retain nonterminal or unimplemented scope. |
| cas-1d5c | P1 | live | open | All-status child audit: 21 children, {'closed': 19, 'open': 2}; retain nonterminal or unimplemented scope. |
| cas-5de5 | P1 | live | open | All-status child audit: 17 children, {'closed': 10, 'open': 7}; retain nonterminal or unimplemented scope. |
| cas-4004 | P1 | obsolete | cancelled | QA child of cas-f2ad, parent status closed; already-shipped delivery needs supervisor qa_reconcile, not a new QA verdict. |
| cas-c10b | P1 | shipped | closed | All-status child audit: 6 children, {'closed': 5, 'cancelled': 1}; terminal candidate; supervisor owns branch-content disposition. |
| cas-b2d8 | P1 | shipped | open | v3.85.0; `331f150a48c8`; art-c469d6bf. Midnight timestamp defect fixed by 331f150a4 (v3.85); existing cas-9912 proof art-c469d6bf covers pinned midnight windows and affected modules. Source: `crates/cas-store/src/reminder_store.rs:423`. |
| cas-d82b | P1 | live | open | Source guidance is not the required three-session eight-metric post-release measurement; retain. Source: `cas-cli/src/builtins/skills/cas-supervisor.md:27`. |
| cas-7a33 | P1 | live | open | Hosted response policy and media playback need deployment-owner proof; local source cannot prove hosted acceptance. |
| cas-0ad2 | P1 | shipped | open | v3.86.0; `98e63a29095a`; art-d991581f. 98e63a290 (v3.86) clears all selectors before binding synthetic child homes; cas-cba9 and cas-5b47 receipts cover real path and hostile ambient selectors. Source: `cas-cli/tests/factory_mcp_ops_test.rs:396`. |
| cas-b9d4 | P1 | live | open | All-status child audit: 19 children, {'closed': 18, 'open': 1}; retain nonterminal or unimplemented scope. |
| cas-5b50 | P1 | shipped | open | All-status child audit: 42 children, {'closed': 35, 'cancelled': 7}; terminal candidate; supervisor owns branch-content disposition. Hold: prowl verification owner ef2c9a21; recommendation only. |
| cas-c439 | P1 | live | open | 14 days of measured usage required; created Oct26 and still too early on Nov1. Retain measurement hold. Hold: operator/measurement/live-proof hold; unchanged. |
| cas-4b74 | P1 | live | open | Operator explicitly parked proposal Oct26; do not staff or resolve during sweep. Source: `.github/workflows/lane.yml:4`. Hold: operator/measurement/live-proof hold; unchanged. |
| cas-ebaa | P1 | live | open | Authorized dedicated-profile discovery remains a live operator-owned proof obligation; no account access performed. Hold: operator/measurement/live-proof hold; unchanged. |
| cas-2455 | P1 | live | open | Certificate/provisioning plus two native signed-update proofs remain owner-gated; do not infer acceptance from release age. Source: `.github/workflows/release.yml:15`. Hold: operator/measurement/live-proof hold; unchanged. |
| cas-4931 | P1 | live | open | Operator direction leaves all seven dependency-security children unstaffed and untouched; not audited for closure. Hold: security hold. |
| cas-d67c | P1 | live | open | Operator direction leaves all seven dependency-security children unstaffed and untouched; not audited for closure. Hold: security hold. |
| cas-fb94c | P1 | live | open | Operator direction leaves all seven dependency-security children unstaffed and untouched; not audited for closure. Hold: security hold. |
| cas-f989 | P1 | live | open | Remaining live-device/operator/two-machine checklist needs real receipts; source presence cannot satisfy it. Source: `docs/qa/journeys.yaml:10`. Hold: operator/measurement/live-proof hold; unchanged. |
| cas-cfbb | P1 | live | open | Live migration plan art-a566ec03 still requires operator authorization and queue/conflict readback; leave untouched. Source: `cas-cli/src/cloud/project_aliases.rs:36`. Hold: operator/measurement/live-proof hold; unchanged. |
| cas-1979 | P1 | live | open | All-status child audit: 53 children, {'closed': 46, 'blocked': 1, 'cancelled': 6}; retain nonterminal or unimplemented scope. |
| cas-7fdd | P1 | live | open | All-status child audit: 0 children, {}; retain nonterminal or unimplemented scope. |
| cas-6a39 | P2 | live | open | Candidate promote/decline decision not recorded; retain supervisor decision work. Source: `cas-cli/src/builtins/skills/cas-supervisor.md:14`. |
| cas-6ad1 | P2 | live | open | Explicit next no-code design; cannot implement auto-close until design and operator decision exist. Source: `cas-cli/src/builtins/skills/cas-cut-release/SKILL.md:3`. |
| cas-0f55 | P2 | live | open | Recent repeated supervisor-owned verification alerts remain reproduced; prior suppression fixes do not prove this recurrence resolved. Source: `cas-cli/src/ui/factory/director/events.rs:38`. |
| cas-c93a | P2 | live | in_progress | QA child of cas-77872, parent status awaiting_merge; active new adversarial pass; leave running review alone. |
| cas-fdef | P2 | live | open | Session-lifetime attribution remains independently open; source removal of ambient recall does not rewrite historical counts. Source: `cas-cli/src/quality_ledger.rs:342`. |
| cas-225d | P2 | live | open | 3.96/prowl scope: recorded keyboard-named file refusal remains; no changes. Source: `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:1164`. Hold: 3.96/prowl or split 3.97 scope. |
| cas-c28c | P2 | live | open | Real 3.94/3.95 reports omitted queue ejection; no backfill/count proof found. Retain. Source: `cas-cli/src/quality_ledger.rs:118`. |
| cas-39d8 | P2 | live | open | Four generated unsupported login calls remain; root CLI error-message repair did not fix executable templates. Source: `scripts/provision-hetzner.sh:180`. |
| cas-706a | P2 | obsolete | open | Supervisor already designated duplicate cas-4069; f0da391ed repairs stream-abort binding, 12400 passes in art-4d702ad5. Cancel duplicate. Source: `hub-web/src/connection.ts:6`. |
| cas-fd2a | P2 | live | open | Draft text fixed but release cut-path enforcement issue not established fixed; retain actual workflow defect. Source: `scripts/release-gate.sh:4`. |
| cas-490b | P2 | live | open | Four no-caller verdicts/wire-or-delete changes remain required; audit note is not completed implementation. Source: `cas-cli/src/ui/factory/daemon/runtime/violet_reader.rs:270`. |
| cas-5b72 | P2 | live | open | Absent on main as reported, but defect specifically targets stale test-reliability branch; do not close from main absence alone. |
| cas-5a657 | P2 | live | open | Selected provider mark needs measured 3:1 forced-colors identity journey capture; generic forced-colors fixes do not establish it. Source: `hub-web/src/styles.css:54`. |
| cas-a147 | P2 | live | open | Operator explicitly accepted current Team view and assigned real same-state craft miss here; retain remaining 4/5 requirement. Source: `hub-web/src/team-view.test.ts:53`. |
| cas-238e | P2 | live | open | Loaded port-collision attribution and required unique-port proof remain; isolated passes do not close flake. Source: `cas-cli/tests/hub_clean_home_test.rs:414`. |
| cas-7ef5 | P2 | live | open | No real 390px before/after showing every header control fits; retain visible phone issue. Source: `hub-web/src/styles.css:1955`. |
| cas-31e6 | P2 | live | open | Installed version alone does not prove guidance, recording-cost measurement or bounded healer trial; retain. Source: `cas-cli/src/builtins/skills/cas-playwright-debug/SKILL.md:16`. |
| cas-5e79 | P2 | live | open | Current source has correct wording and no capability probe, but native macOS acceptance/which implementation survives branch sync is unverified; retain conservatively. Source: `cas-cli/src/skill_validation.rs:17`. |
| cas-f702 | P2 | live | open | Existing multi-run support is not proof of real pytest XML matching and collision-free Playwright copies; retain runner follow-up. |
| cas-4336 | P2 | live | open | Native macOS canonical-path comparison proof remains required; Linux cannot establish acceptance. Source: `cas-cli/src/ui/factory/director/events_tests/tests.rs:3806`. |
| cas-013b | P2 | live | open | Remote-only target-ref fixture and accurate missing-ref diagnostic not verified; retain. Source: `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/receipt_scope.rs:187`. |
| cas-68bd | P2 | obsolete | open | Superseded by shipped cas-e0d4 issue-backed fail/pass recovery and second-occurrence quarantine, with operator-approved hosted proof art-70fde6f9. Source: `scripts/ci-queue-flakes.py:58`. |
| cas-db56 | P2 | live | open | Clock-only snapshot recovery is shipped, but advisory+QA-deferral self-mutation case needs its own proof; retain distinct behavior. Source: `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs:13`. |
| cas-cde3 | P2 | live | open | All-status child audit: 41 children, {'awaiting_merge': 1, 'in_progress': 2, 'closed': 36, 'cancelled': 2}; retain nonterminal or unimplemented scope. |
| cas-d3da | P2 | live | open | Adjacent DESIGN path@revision still goes through literal local-path resolution; retain independent false receipt defect. Source: `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/proof_claims.rs:77`. |
| cas-caf5 | P2 | shipped | closed | All-status child audit: 45 children, {'cancelled': 4, 'closed': 41}; terminal candidate; supervisor owns branch-content disposition. |
| cas-9d08 | P2 | shipped | closed | All-status child audit: 57 children, {'closed': 53, 'cancelled': 4}; terminal candidate; supervisor owns branch-content disposition. |
| cas-6163 | P2 | live | open | Original loaded capture failure preserved; isolated pass and generic contention fixes do not identify this delay. Retain. Source: `hub-web/e2e/journeys/installation.journey.ts:36`. |
| cas-d587 | P2 | shipped | open | v3.92.0; `e534d3b20185`; qab-fbb74be095834b00bf43e02cce3d2230. e534d3b20 and merge a86328e33 shipped v3.92; batch QA7 R26 HUB-J4 PASS and 51 vitests recorded on task; stranded anchor prevents admin close. Source: `hub-web/src/conversation-view.ts:69`. |
| cas-297b | P2 | live | open | Six retained failures need same-command loaded attribution; individual readiness repairs cannot establish every remaining cause. Source: `hub-web/e2e/journeys/launch-session.journey.ts:294`. |
| cas-97fc | P2 | live | open | Contradictory header/retry state still needs explicit regression and derivation proof; retain. Source: `hub-web/src/conversation-view.ts:1754`. |
| cas-1e0d | P2 | live | open | Symlink-only unlink authorization not proven; preserve target protection and keep live. Source: `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:988`. |
| cas-c1ba | P2 | live | open | Code-frame feature shipped, but one shared frame for diagrams/diffs/code plus measured density and motion proof remain incomplete. Source: `hub-web/src/work-rows-view.ts:13`. |
| cas-8f81 | P2 | live | open | Hard-coded multiplication help glyph remains on main; retain C-locale defect. Source: `cas-cli/src/cli/factory/mod.rs:1041`. |
| cas-1060 | P2 | shipped | open | v3.88.0; `f02bfbf7565d`; art-ef726eff. Shipped cas-2850/01657cbf3 (v3.88) syncs independent QA and evidence; corrective f02bfbf75 covers task-bound exports, remote paths and cache; art-ef726eff has 866 targeted passes. Source: `cas-cli/src/cloud/syncer/pull.rs:1489`. |
| cas-f16c | P2 | live | open | Restored phone thread readiness after reload needs original failing-spec control; retain. Source: `hub-web/e2e/journeys/interrupt-raw-output.journey.ts:59`. |
| cas-3dce | P2 | live | open | Catalog-before-outage readiness not proved for this recurrence; retain source-specific fixture defect. Source: `hub-web/e2e/journeys/launch-session.journey.ts:305`. |
| cas-49e4 | P2 | shipped | open | v3.85.0; `331f150a48c8`; art-c469d6bf. Same midnight timestamp bug as cas-b2d8; 331f150a4/v3.85 fixes instant comparison and preserves old-history exclusion; existing art-c469d6bf is exact test evidence. Source: `crates/cas-store/src/reminder_store.rs:423`. |
| cas-21c4 | P2 | obsolete | open | QA child of cas-326e8, parent status closed; already-shipped delivery needs supervisor qa_reconcile, not a new QA verdict. |
| cas-6adaa | P2 | live | open | MIME allowlist and Cloud cost decision remain explicit operator/product work; media renderer alone does not satisfy publishing acceptance. Source: `cas-cli/src/artifacts/cloud.rs:48`. Hold: operator/measurement/live-proof hold; unchanged. |
| cas-9fe5 | P2 | obsolete | open | 800ebb361 fixed selector isolation, then 6253627f6 retired entire release-train test/script; obsolete acceptance targets, no replacement repair needed. |
| cas-0ef8 | P2 | live | open | Related cas-dd72 is closed on 3.97 but has not shipped to origin/main; native 19-fixture acceptance remains live here. Source: `cas-cli/src/hub/observation.rs:477`. |
| cas-11c0 | P2 | live | open | inventory still enumerates only explicit targets while count supports autodiscovery; legitimate cas-factory harness admission remains unproved. Source: `scripts/cas-test-targets.py:38`. |
| cas-33f6 | P2 | live | open | Current-source build resolver is fixed, but verifier runner explicitly leaves Zig discovery to checked-out source; older tips lacking resolver remain unproved. Retain older-tip support requirement. Source: `cas-cli/src/factory_worker_check.rs:3566`. |
| cas-3388 | P2 | live | open | cas-711a fixed grouped bold counting, but is_non_shipped_category excludes only test/doc/known limit and still counts Upgrade notes. Retain partial issue. Source: `cas-cli/src/cli/release_report.rs:1248`. |
| cas-f0f3 | P2 | live | open | Detached linked-worktree isolation warning needs real my_context regression; branch absence alone is not proof. Retain. Source: `cas-cli/src/factory_isolation.rs:107`. |
| cas-b181 | P2 | live | open | Read-only literal-key documentation text false refusal not proven fixed by shell bypass work; retain. Source: `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:5429`. |
| cas-d7ea | P2 | live | open | Authorized cross-epic assembly itself shipped, but generic allowed/refused park regressions remain unverified; retain workflow defect. Source: `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/task_attribution.rs:23`. |
| cas-7b24 | P2 | live | open | Opaque WebSocket ticket query scrubbing needs actual trace regression; generic redaction is insufficient. Retain. Source: `scripts/visual-qa.mjs:12`. |
| cas-be50c | P2 | shipped | open | v3.81.0; `3a00059ef68d`; art-6d086982. 3a00059ef/fad204a3b (v3.81) default filtered task waits to TTL0 and emit explicit-expiry notice; real scanner regression and 612 affected tests in art-6d086982. Source: `cas-cli/src/mcp/tools/service/factory_remind.rs:18`. |
| cas-5cac | P2 | live | open | All-status child audit: 48 children, {'closed': 15, 'open': 26, 'blocked': 5, 'cancelled': 2}; retain nonterminal or unimplemented scope. |
| cas-dbd0 | P2 | live | open | Natural failure/repair run pair remains unmeasured; do not synthesize a failure just to obtain timing. Source: `.github/workflows/lane.yml:4`. |
| cas-479ad | P2 | live | open | Source fix shipped but live supervisor /clear identity and peer receipt remain required; worker cannot drive them. Source: `cas-cli/src/mcp/daemon.rs:286`. |
| cas-8320 | P2 | live | open | Cloud shared-admission and authorized real two-supervisor interval/transfer proof remain outstanding; retain owner hold. Source: `cas-cli/src/coordination/stakeholder_contacts.rs:335`. Hold: operator/measurement/live-proof hold; unchanged. |
| cas-07d1 | P2 | live | open | Operator direction leaves all seven dependency-security children unstaffed and untouched; not audited for closure. Hold: security hold. |
| cas-3eb6 | P2 | live | open | Operator direction leaves all seven dependency-security children unstaffed and untouched; not audited for closure. Hold: security hold. |
| cas-b9b0 | P2 | live | open | Operator direction leaves all seven dependency-security children unstaffed and untouched; not audited for closure. Hold: security hold. |
| cas-2907 | P2 | live | open | All-status child audit: 7 children, {'open': 7}; retain nonterminal or unimplemented scope. |
| cas-307e | P2 | live | open | Narrow-refspec destination refresh remains distinct from default-refspec incidents; retain real-Git regression requirement. Source: `cas-cli/src/worktree/git/branch_ops.rs:342`. |
| cas-5bb5 | P2 | live | open | Unanswered operator decision on explicitly stopped services persists; leave untouched. Source: `cas-cli/src/cli/update.rs:2934`. Hold: operator/measurement/live-proof hold; unchanged. |
| cas-6bca | P3 | live | open | Entry is still under 3.94 while source retirement first appears in 3.95; retain precise docs correction. Source: `CHANGELOG.md:145`. |
| cas-3720 | P3 | live | open | Unregistered identity recovery message not proved corrected through actual dispatch; retain. Source: `cas-cli/src/mcp/server/mod.rs:512`. |
| cas-cd3eb | P3 | live | open | New 3.93 incident targets aliased active memory path; older generic memory allowance is insufficient evidence. Retain. Source: `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:959`. |
| cas-8a36 | P3 | live | open | Already-answered blocker recurrence remains distinct from older suppression contract; retain with newer cas-0f55 related, no speculative duplicate cancellation. Source: `cas-cli/src/ui/factory/director/events.rs:38`. |
| cas-1387 | P3 | live | open | Duplicate proof notes observed on three deliveries; snapshot causality remains unproved, preserve both behaviors for diagnosis. Source: `cas-cli/src/mcp/tools/core/artifact.rs:43`. |
| cas-c053 | P3 | obsolete | open | QA child of cas-89b0, parent status closed; already-shipped delivery needs supervisor qa_reconcile, not a new QA verdict. |
| cas-6c60 | P3 | obsolete | open | Exact documented target=self refusal duplicates older cas-d398; canonical-name/omitted-target workaround does not resolve defect. Source: `cas-cli/src/mcp/tools/service/factory_remind.rs:298`. |
| cas-c74f | P3 | live | open | cas-fc4c fixed pending reread and HUB-J25 return; phone name wrapping/alignment criterion remains unproved. Retain partial follow-up. Source: `hub-web/src/work-picture.ts:117`. |
| cas-d398 | P3 | live | open | Canonical older target=self bug remains; no regression through reminder dispatch proving self alias found. Source: `cas-cli/src/mcp/tools/service/factory_remind.rs:298`. |
| cas-f25d | P3 | obsolete | open | QA child of cas-310f, parent status closed; already-shipped delivery needs supervisor qa_reconcile, not a new QA verdict. |
| cas-9474 | P3 | live | open | cas-bb86 fixes original ENOTEMPTY, but late log-root recreation is unproven distinct symptom; retain until writer ownership captured. Source: `hub-web/e2e/journeys/real-hub-server.mjs:165`. |
| cas-e1d3 | P3 | live | open | Doc-comment and disconnected-response bounding decisions both required; no complete record found. Retain. Source: `crates/cas-store/src/event_store.rs:240`. |
| cas-0b56 | P3 | live | open | Dead resume plumbing removal and repeated usage-report stored-text behavior remain distinct unproved criteria. Retain. Source: `crates/cas-mux/src/mux.rs:1007`. |
| cas-017f | P3 | live | open | Unborn fixture-repo add/commit acceptance not proved; detached-worker fence must remain. Retain. Source: `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:3045`. |
| cas-2ecf | P3 | live | open | 3.96 liveness changes are not on main and do not prove selected-row/header consistency; retain. Source: `hub-web/src/worker-visibility.ts:10`. |
| cas-bbf4 | P3 | live | open | Writer distinguishes fields, but actual post-adapter regression at scripts/test-release-report-post.py:803 still expects a file URL for PDF_MESSAGE_PERMALINK. Keep real message-permalink defect open. Source: `scripts/test-release-report-post.py:803`. |
| cas-9a15 | P3 | live | open | Both copies still say backspace removes cell, contrary to cursor behavior; retain tiny doc task. Source: `scripts/terminal-qa.mjs:288`. |
| cas-ecfd | P3 | live | open | Release-flow design and explicit approval remain required; no new gate authorized. Leave proposal untouched. Source: `scripts/release.sh:3`. Hold: operator/measurement/live-proof hold; unchanged. |
| cas-8da5 | P3 | live | open | Raw decorative push counters remain; shared fallback does not prove callers use it. Retain C-locale renderer defect. Source: `cas-cli/src/cli/cloud.rs:2955`. |
| cas-52e6 | P3 | live | open | macOS holder capture or bounded negative runs required; Linux controls cannot close. Retain prowl work. Source: `cas-cli/src/qa_registry.rs:1305`. |
| cas-3216 | P3 | live | open | Operator direction leaves all seven dependency-security children unstaffed and untouched; not audited for closure. Hold: security hold. |
| cas-2cf4 | P3 | live | open | Named helpful-memory hygiene test still exists on main; native macOS output-holder proof remains required. Retain, with runner-level investigation cas-799e. Source: `cas-cli/tests/session_start_memory_hygiene_test.rs:262`. |
| cas-62fc | P3 | live | open | Native macOS stdout/stderr holder diagnosis still required; passing assertions with LEAK are not resolved. Retain prowl work. Source: `cas-cli/src/ui/factory/daemon/runtime/shutdown_tests.rs:675`. |
| cas-2754 | P3 | live | open | Help-width source changed since discovery, but no isolated 80-column rendered comparison established; retain uncertainty. Source: `cas-cli/src/cli/hub.rs:19`. |
| cas-53d1 | P3 | live | open | Conditional follow-up younger than required 30 days; no recurrence capture available, do not close early. Source: `cas-cli/src/cloud/task_proposals.rs:1331`. |
| cas-d074 | P3 | live | open | Read-only version exception not proved through guard; do not execute forbidden Cargo command to test it. Retain. Source: `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs:1814`. |
| cas-aa18 | P3 | live | open | Invalid-bind pre-stop validation not proved; avoid mutating live hub during no-code sweep. Retain. Source: `cas-cli/src/cli/hub.rs:107`. |
| cas-799e | P4 | live | open | Runner-level native macOS holder capture remains owned by prowl; source-level negative controls are insufficient. Source: `cas-cli/src/ui/factory/daemon/runtime/shutdown_tests.rs:1797`. |
| cas-0e5e | P1 | live | open | New during sweep: operator screenshot shows blank header/code-like table; existing table feature does not prove new screenshot defect fixed. Retain on split 3.97 board; no changes. Hold: new protected board work. |
| cas-a929 | P2 | live | open | New during sweep: attention badge clips 390px phone header in native candidate QA; retain under 3.96/prowl scope. Hold: new protected board work. |
| cas-6a80 | P2 | live | open | New during sweep: default cached headless shell crashes on native conversation attach; full Chromium control passes. Retain native browser-identity investigation under 3.96/prowl scope. Hold: new protected board work. |

## Acceptance and limits

- **VERIFIED:** all122 startup IDs and all3 cutoff additions have exactly one row; the125 dispositions sum to12 shipped,8 obsolete,105 live. Coverage validation fails on duplicates or missing IDs.
- The release-tag validator caught and corrected a provisional `cas-0ad2` release mapping (candidate-era3.82 versus first-containing tag3.86) before delivery.
- **VERIFIED:** every ordinary shipped candidate's cited commit is an ancestor of pinned main and its first-containing release tag was checked. The typed-work source equivalence and retained-source locations were inspected directly.
- **VERIFIED:** administrative refusals are recorded verbatim, all-status epic census preserves nonterminal work, and held/protected rows were not changed by the worker.
- **VERIFIED:** the ready checkpoint decreased122→118, with exact terminal/active/new-row accounting above. Supervisor close reasons and current task statuses establish four terminal rows; worker close attempts themselves completed zero source-task closes.
- **NOT VERIFIED:** fresh Rust/browser/native-Mac behavior, live external-profile or device acceptance, and model actions after a transport acknowledgement. Their existing receipts or named owners remain explicit.
- **NOT VERIFIED:** remaining supervisor-only cancellation/legacy-ledger/stranded-anchor dispositions and prowl's Accounts close. They are pending administrative work, not claimed completed by this report.

Design: `DESIGN.md@b43940e43`, inherited `hub-web/DESIGN.md`; plain Markdown uses the project's quiet, precise voice. No product code, tests, release gates, version, CHANGELOG or public UI changed.

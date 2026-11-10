# Cassy's whack-a-mole is 388 factory checks, not the release flow. Five releases cut them to 274, and each removes more than it adds

{{figure:hero}}

The release pipeline cas-a677 redesigned now meets its budget: over the last 8 releases, PR opened → merged took a median 19.8 min and merged → published 8.0 min (7 of 8 within 20). The pain is the layer around it: 388 gates, guards, watchdogs, notices and CI checks. Overnight they cost 36 hand-written overrides, a 9 h 30 min installer hang no alarm caught, and 98 blocker wakes answered "Ignored." 90 times.

## The decision

**Recommendation: approve the four moves and the five-release sequence below.** The operator decides; soundwave reviews this design before it goes to the operator. Rule 176 (1) and (3) hold: the plan derives from cas-a677's whole-flow map and the approved 4.13 close-at-release design, and every release removes more checks than it adds.

| Move | What it changes | Release |
| --- | --- | --- |
| 1. Release reconciler (approved) | One operation keyed on the release SHA closes shipped work, the train and finished epics | 4.13 |
| 2. Facts are the single source of truth | Notices, watchdogs and Commander views derive from recorded facts, one dedup key each | 4.15 |
| 3. Capability guards | OS sandbox, scoped credentials and branch rules replace shell-text parsers | 4.16 |
| 4. Gate budget and generated docs at the cut | Stable gate codes, as-of approval, one in for one out; generated docs built at the cut | 4.14, 4.17 |

## The diagnosis, tested

The operator's question was whether Cassy is built sanely, or whether we are playing whack-a-mole with gates. The brief named three causes to test rather than assume. All three hold; one needs a correction.

| Claim | Verdict | Headline evidence |
| --- | --- | --- |
| Gates police actions against moving state: branch tips, current rules, shell text | **Holds** | 84 of 127 gates read moving state, 49 of them after the decision is made; 22 of 35 PreToolUse guards parse shell text |
| Each incident adds a gate and none are removed | **Holds** | the stall watchdog got three one-off exemptions in four days; `close_ops.rs` grew from 39,390 to 46,913 lines since October 15 |
| State is split across stores, each with its own notices | **Holds, with a correction** | the immutable facts exist; the release fact and the derive-every-notice rule are missing; 71 of 174 watchdogs and notices re-poll live state |

<details open data-collapse>
<summary>Evidence for each claim</summary>

- **Moving state.** 84 of 127 close and merge gates read a moving ref, current rule or the live task row, including 49 of the 79 that run after the decision is made: park, close after merge and epic close (appendix A). The delivery state machine has two states that exist only because the target moved: `Stale` and `TipChanged` (`crates/cas-types/src/delivery.rs:61-63`). Tonight's 36 hand-written content-drop overrides re-compared shipped deliveries against today's target ref (I-13). Gates added after approval refused already-approved work (I-10). 22 of 35 PreToolUse guards parse shell text (appendix B).
- **Add, never remove.** The stall watchdog was patched three times in four days with one exemption each (I-05, I-06, I-07). Of cas-a677's October 15 delete list, the release train and the daemon sweeps went (−10,658 Rust lines), but the local gate, assembly proof and bookkeeping scripts remain or grew; script tests grew from 27,228 to 28,672 lines and `ci.yml` from 1,877 to 2,653 (appendix E).
- **Split state.** The immutable facts already exist: completion receipts, append-only delivery events, observed merges and QA batch receipts (`crates/cas-store/src/delivery_store.rs:18,60,80`; `qa_batch_store.rs:10`). What is missing is the **release** fact, and the rule that every notice is derived from facts instead of re-polling live state. 71 of 174 watchdogs and notices re-poll live state, 6 of them for a fact the store already records (appendix C and D). The 9.5 h installer hang (I-15) and the 98-wake transcript-mirror flood (I-16) are both a fact that no one recorded: "published X, installed Y" and "this mirror belongs to an ended session".
- **Correction to the brief.** No verification-dispatch flood was found in prowl's store (35 dispatches overnight, at most 3 per task; a unique active-dispatch index exists at `crates/cas-store/src/verification_store.rs:246`); soundwave's store was not read. The flood that did happen was the transcript-mirror one (I-16).

</details>

## What the operator paid tonight

All times Eastern, November 9–10. The full ledger of 21 incidents in 6 classes is in appendix F.

| Incident | Cost | Class | Move that removes it |
| --- | --- | --- | --- |
| Content-drop refusals after each release (I-13) | 36 hand-written supervisor overrides on 34 tasks, 7:41 PM–7:50 AM; 28 named only changelog fragments, CODEMAP, journeys.yaml or PRODUCT_OVERVIEW | generated-doc conflict + false positive | moves 1 (release proves supersession once) and 4 (generated docs built at cut) |
| journeys.yaml merge conflicts (I-14) | 16 of 42 merges conflict as plain text; 4 still conflict with the new merge driver; 5 deliveries returned to workers | generated-doc conflict | move 4 |
| Installer hang (I-15) | 9 h 30 min, 11:11 PM–8:42 AM; v4.12.0 about 1 h late on prowl; no watchdog fired | missing fact | move 2 (a "released, not installed after 20 min" deadline on recorded facts) |
| Transcript-mirror blocker flood (I-16) | 98 wakes; 90 "Ignored." replies | missing fact | move 2 |
| Stall watchdog false pages (I-05–I-07) | 5 false notices, 2 threatened pages, 3 finished tasks set to blocked by hand to stop a page | missing fact | move 2 |
| Guard false positives (I-01–I-03, I-18, plus five hit while writing this) | GH #1752, #1615, #1754; a fix carried across 3 trains; blocked replay and diagnosis work | false positive | move 3 |
| Retroactive gates (I-10) | 2 finished QA tasks cancelled instead of closed | moving-state race | move 4 (gates apply as of approval) |

<details open data-collapse>
<summary>Five guard false positives on this spike's own read-only work</summary>

Five guard false positives hit this design spike's own read-only work, two of them in research agents:

1. About 9:43 AM ET, the operator-credentials guard denied `find /Users/pippenz -maxdepth 6 -type d -name release-flow-review`, which reads directory names, not Neon or Vercel stores.
2. About 9:55 AM ET, the worker tool denial refused the harness message tool for this worker's own read-only research subagents. Four of them had to be stopped and relaunched to correct their source pin.
3. About 9:50 AM ET and again about 9:57 AM ET, the workspace-contract shell parser refused read-only loops because they contained `$((n+1))`: a merge-counting loop in the incident research (I-18) and this spike's `until ls …; do sleep 20; done` wait loop.
4. At 10:06 AM ET, the operator-credentials guard refused the heredoc that wrote the guard inventory into the task's artifact directory, because the inventory's text names the credential store paths (`operator_credentials.rs:158-160`). The same bytes passed through the Write tool, which that guard does not inspect.

</details>

## Inventory: every gate, guard, watchdog and notice

Every row cites `file:line` at `01df2dc77` (v4.12.0); the full tables are in the appendix. Verdicts: **keep** what protects integrity or is the only signal for a real failure; **merge** what re-checks a fact another row or the reconciler already owns; **delete** what duplicates CI, has no recipient, or is replaced by a capability.

Table: Inventory by kind and verdict (rows; "reads moving state" = re-reads a branch tip, target ref, current task row, live GitHub or Cloud; for guards, parses shell text)

| Kind | Rows | Keep | Merge | Delete | Move or decide | Reads moving state or shell text |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Close, park, merge and release gates (G) | 127 | 88 | 31 | 8 | 0 | 84 |
| Agent guards: hooks, git hooks, role checks (H) | 46 | 16 | 13 | 17 | 0 | 22 |
| Watchdogs (W) | 69 | 44 | 24 | 1 | 0 | 44 |
| Notices (N) | 105 | 89 | 13 | 3 | 0 | 27 |
| CI and release checks (C) | 41 | 27 | 5 | 6 | 3 | 0 |
| **Total** | **388** | **264** | **86** | **35** | **3** | **177** |

<details open data-collapse>
<summary>What the inventory shows, kind by kind</summary>

What the inventory shows, kind by kind:

- **Gates.** `close_ops.rs` is 46,913 lines (about 23,200 production) with 553 inline tests, and 515 commits touched it since October 15. Refusals are free-text headings, not codes: `MERGE REQUIRED` appears 28 times, and the typed gate error has 12 variants for 127 gates. One gate is the hub of the churn: the merge-state gate and its content check (G-55, G-56) re-measure the current branch tip and target on every close, including Cassy's own post-merge close; the comments at `close_ops.rs:17229-17560` are six layered fixes for that. A near-reconciler already exists (G-123, G-90) but re-runs the ordinary gates. Review is enforced before merge (G-104) and again after (G-67–G-70). Four proof gates duplicate the merge queue (G-78–G-80, G-63). Park, close-after-merge and epic close hold 79 gates, 49 of which re-read moving state after the decision was made.
- **Guards.** 22 of 35 PreToolUse guards parse shell text through 25 separate recognizers. `pre_tool.rs` grew from 8,938 to 13,824 lines since October 15 across 213 commits, and 26 commit subjects say "bypass": fixes for bypasses and fixes for false positives alternate. Of 23 guard false positives in 30 days (18 filed issues, from #1218 to #1754, plus 5 hit while writing this design), 21 hit guards that parse shell text; the workspace write guard alone has 11. For most of them the capability already exists in the codebase: Linux credential masking (`crates/cas-pty/src/store_isolation.rs:1-18`), CLI operator checks with a TTY and typed confirmation (`cas-cli/src/cli/config/operator_ops.rs:75-92`), MCP supervisor-only routes (`cas-cli/src/mcp/server/runtime.rs:410-416,623-633`) and per-worktree git hooks that the code itself calls "the hard floor" (`pre_tool.rs:2338-2339`).
- **Watchdogs and notices.** Three stall detectors key on absence of activity with different dedup keys, so one quiet night can page the operator twice (W-01/W-02, W-03/W-04, W-05–W-10). "Merged, not closed" has four reporters with four keys (W-45–W-47, W-51). A park is announced twice (N-24, W-48). One unread message can raise four notices and two operator escalations (W-31, W-28, W-33, W-69, W-34). 46 rows have no dedup; 71 re-poll live state, 6 of them for a fact the store already records.
- **CI and release.** Workflows went from 7 to 8 and `ci.yml` from 1,877 lines and 19 jobs to 2,653 and 24 since October 15. Of cas-a677's delete list, the release train, the daemon sweeps (−10,658 lines) and the parity pins are gone; release-gate.sh, assembly proof, the memory scripts and the bookkeeping scripts remain or grew, and 887 lines of release scripts have no production caller. Train branches ran the full set 177 times across the last 8 trains (123 cancelled, 39 failed, about 1,639 wall-minutes) against one queue run per release.
- **Generated docs.** journeys.yaml, PRODUCT_OVERVIEW.md and CODEMAP.md are the three most-conflicted files since October 27 (in 10, 8 and 7 of 43 conflict lists; no source file appears more than 3 times), because park gates G-49 and G-50 make every delivery rewrite them.

</details>

## Target architecture

### Move 1 (given): one release reconciler keyed on an immutable SHA

The approved 4.13 design (`cas-47b2/close-at-release-design.md`) stands as written; release 4.13 below counts what it deletes.

### Move 2: facts are the single source of truth

**Rule.** A gate, notice, watchdog or view reads only recorded facts and one liveness signal, never a branch tip, current rule, GitHub page or peer's table, to decide what happened.

<details open data-collapse>
<summary>The fact model: five facts exist today, three are added</summary>

Table: Fact kinds. Five already exist; three are added.

| Fact | Key | Written by | Exists today |
| --- | --- | --- | --- |
| Delivered | task, cycle, commit SHA, base SHA | worker close | yes: `worker_completion_receipts`, immutable (`delivery_store.rs:18-41`) |
| Delivery event | transaction, state | close and merge paths | yes: `worker_delivery_events`, append-only (`delivery_store.rs:60-75`) |
| Merged | task, target, source SHA, merge SHA | worktree_merge | yes: `observed_delivery_merges`, immutable (`delivery_store.rs:80-97`) |
| Verified | task, cycle, verdict type, artifact digest, author | verifier, batch QA | yes: `verifications`, `qa_batch_receipts` (`verification_store.rs:158`; `qa_batch_store.rs:10`) |
| Released | project, release ID, final SHA, member set | the 4.13 reconciler | **new** (4.13 release context) |
| Installed | host, release ID, binary SHA | install-on-publish | **new**; today a free-text release-note edit |
| Held | subject, gate ID, as-of SHA, owner, next action | any refusing gate | **new**; today refusal text in task notes |
| Closed | task, cycle, as-of release SHA | the reconciler | partly: `tasks.status` plus `close_reason` text |

Every fact carries `as_of`: the immutable SHA, release ID or rule-set version it was decided against. A fact never changes; a later fact supersedes it.

</details>

**Derived notices.** A notice is a pure function of facts, `notice = project(fact, audience)`, with one dedup key `(subject, fact kind, as_of)` and one outbox; it resolves when a superseding fact arrives, as a `Closed` resolves the `Held` of the same subject. Commander, session-start context and Slack read the same projection, so they cannot disagree.

<details open data-collapse>
<summary>Where today's notices disagree</summary>

Today codemap freshness is computed twice, at SessionStart and at Stop (`stop_flow.rs:753-755`), from two sources that can disagree. Commander gives every daemon notice one kind, `delivery_stall`, and the headline "The supervisor missed an update" (`hub-web/src/operator-notices.ts:12`; `hub-web/src/attention.ts:229-234`), and it regex-parses the message prose to find a worker name.

</details>

**Watchdogs collapse into three monitors.**

1. **Fact-triggered notices.** Written on fact insert, never by polling.
2. **One liveness monitor.** Heartbeat age and process exit only. Keeps worker-died, parent-watchdog and daemon-loop health.
3. **One deadline monitor.** "An expected fact is absent by T", with T derived from facts: `Released` with no `Installed` after 20 min would have paged at about 11:31 PM, not 8:42 AM, for I-15.

<details open data-collapse>
<summary>Why the stall heuristics go</summary>

Stall heuristics that infer "nobody is working" from absent activity are deleted. Under this rule a task parked behind a frozen train, a peer's batch receipt or a refused close carries a `Held` fact with an owner, so it is not "stalled" (I-05–I-07).

</details>

<details open data-collapse>
<summary>Cross-machine facts</summary>

**Cross-machine.** Facts replicate through Cloud append-only and idempotently by fact ID. A view labels a peer "last known at fact N" instead of recreating live items from a stale snapshot.

</details>

### Move 3: capability guards in place of shell-text parsing

**Today the text parser is the jail.** Claude workers are spawned with `--dangerously-skip-permissions` and `--permission-mode bypassPermissions`, and the spawn code says why: "Workers are already constrained by the factory hook/worktree jail" (`crates/cas-pty/src/pty.rs:1375,1386-1395`). Codex workers are spawned with `--yolo`, which bypasses approvals and the sandbox (`pty.rs:1563`). No OS boundary stands behind the hooks: every parser hole is a jail hole, and every false positive stops real work.

**Rule.** Prevent an action by not granting the capability, not by reading the command text. A shell-text parser cannot be both complete and free of false positives; I-01, I-02, I-03 and I-18 are four shapes of that one failure.

<details open data-collapse>
<summary>Capability replacements, guard family by guard family</summary>

Table: Capability replacements

| Guard family today | Capability that replaces it | Who enforces |
| --- | --- | --- |
| Workspace write roots, shell-effects parser, tmp writes | Harness filesystem sandbox: Claude Code sandboxed Bash, Codex `workspace-write` with writable roots set to the worktree and the task artifact dir | OS (Seatbelt on macOS, bubblewrap or Landlock on Linux) |
| Operator-credential path guard (Neon, Vercel) | Worker processes run with a scoped `HOME` that has no credential stores, plus sandbox deny-read on the real store paths | OS and spawn environment |
| Commit and history-write guard, push guard | Workers push only `factory/*`: a worker-scoped GitHub identity plus a ruleset that blocks it from `main`, `epic/*` and `release/*`; local history writes stay free inside the sandboxed worktree | GitHub server |
| Slack transport, publication and denied-tool guards | Tools are absent from the worker's harness and MCP config instead of denied at call time; Cassy MCP checks the caller's registered role server-side | harness config and Cassy MCP |
| Neon SQL guard | A read-only database role for workers | database |

Guards that remain are structured-argument checks inside Cassy's own MCP tools (role, task ownership, artifact path), which see typed input, not shell text.

</details>

### Move 4: a gate budget, and generated docs built once

**Gate registry and budget.** Every check gets a registry row with a stable code, so refusals, overrides and false positives become countable facts. The total is capped at 274 after 4.17: adding a check means deleting one in the same release, or an operator waiver recorded as a fact. Each release cut reports, per code, true catches, overrides and seconds spent; a check with no true catch in three releases is listed for deletion.

**As-of approval.** A delivery cycle records the registry version in force when it was approved; later checks do not apply to it retroactively, except rows marked `integrity` (lost content, task identity, independent review, credentials). This removes I-10.

**Generated docs at the cut.** CODEMAP and PRODUCT_OVERVIEW are generated once at release prep, and the journey registry's per-delivery parts (bug families) move to per-task files compiled there, as CHANGELOG fragments already are. No park gate checks their freshness and no two deliveries edit the same generated file. This removes I-12, I-14 and 28 of 36 overrides in I-13.

<details open data-collapse>
<summary>What a registry row records</summary>

Stable code; the defect it catches, with a true-catch incident; stage; cost in median added seconds, measured from refusal facts; false-positive count, from override and reviewed-drop facts; owner; release added; review date; and whether it is an `integrity` row.

</details>

## Migration: five releases, each with a net reduction

Each release ships only after the previous one is measured on real releases. Row IDs refer to the appendix. "Merged" rows disappear into the named survivor; the survivor keeps its own row.

Table: Check count per release (all five kinds)

| Release | Scope | Removed | Added | Net | Checks after | Of which read moving state or shell text |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Today (v4.12.0) | | | | | 388 | 177 |
| 4.13 | Release reconciler, as approved (move 1) | 17 | 1 | −16 | 372 | 165 |
| 4.14 | Gate registry and as-of approval; generated docs at the cut; duplicates of CI and dead rows (move 4) | 31 | 0 | −31 | 341 | 152 |
| 4.15 | Facts and derived notices; three monitors (move 2) | 45 | 2 | −43 | 298 | 130 |
| 4.16 | Capability guards (move 3) | 19 | 2 | −17 | 281 | 112 |
| 4.17 | One lane job; the queue is the only proof (move 4, CI half) | 7 | 0 | −7 | 274 | 112 |
| **Total** | | **119** | **5** | **−114** | **274** | **−65** |

<details open data-collapse>
<summary>Where the gates go, stage by stage</summary>

Where the gates go: park 50 → 35, close-after-merge 19 → 6, epic close 10 → 1; start (18), merge admission (19 of 20) and release verdict (6) stay. Reading current state is correct when a gate guards the action it is about to take, as start and merge admission do. It is the defect when a gate re-judges a decision that is already recorded, as close-after-merge and epic close do today. The 112 that still read live state after 4.17 are mostly that first kind; the registry from 4.14 tracks each one's false positives so the next cut can see which still misfire.

</details>

<details open data-collapse>
<summary>4.13: the release reconciler (approved; move 1): 17 removed, 1 added</summary>

Builds the approved close-at-release design and nothing more. The reconciler becomes the one gate that closes shipped work, keyed on the release SHA.

- **Removes 17 gates** whose job the reconciler takes over: G-35 (stale-dispatch escape hatch), G-44 (batch-squash receipt), G-53 (implementer QA evidence for batch work), G-56 (content presence, now run once at the release SHA with fragment and generated-file supersession), G-57 (lagging epic receipt), G-64 (independent QA at landing), G-67 (WAITING FOR RELEASE VERDICT), G-71 (epic delivery tip), G-83 (deferred batch QA debt), G-89 (NOT_PARKED), G-90 (release-covered close re-running ordinary gates), G-120 (post-release check), G-123 (installed-tag closer), G-124 (unmerged worker branches), G-125 (per-child epic gate), G-126 (stranded-branch override), G-127 (gone epic branch fallback).
- **Adds 1:** the reconciler, with the new `Released` fact.
- **Ends:** the 36-override night (I-13), the five-step close (gabber cas-d628), and NOT_PARKED holds on shipped work.

</details>

<details open data-collapse>
<summary>4.14: gate registry, generated docs at the cut, free deletions (move 4): 31 removed</summary>

- **Gate registry.** Every remaining gate gets a stable code, so refusals, overrides and false positives can be counted per gate; today there is none to index. A delivery cycle records the registry version at approval, so later gates do not apply retroactively except `integrity` rows (I-10).
- **Generated docs at the cut.** Workers write per-task fragments only. Removes G-50 (CODEMAP and PRODUCT_OVERVIEW drift at park), H-32 (CODEMAP freshness gate), N-99 (CODEMAP note at every stop); G-49 (journey-registry upkeep) moves to the lane lint job.
- **Duplicates of CI and the merge queue:** G-48 (rule references, to the lane lint job), G-63 (test-shape, a fast row), G-78 (platform proof, macOS Check), G-79 (scoped proof), G-80 (build and assembly proof), G-81 (merge reality check). Gate merges: G-77 into G-61, G-82 into G-47, G-98 into G-94, G-115 into G-104. G-72 (legacy worktree jail) goes with H-30 and H-31.
- **Guards already enforced elsewhere:** H-11 and H-13 (the CLI enforces them with a TTY and typed confirmation), H-19 and H-20 (become harness `permissions.deny` entries), H-27 (a workflow condition on the base branch), H-07 (moves into the post handler), H-30 and H-31 (experimental worktree jail, if `worktrees.enabled` is unused). H-21 is narrowed so a worker can message its own subagents.
- **Rows with no recipient or no effect:** W-18 (logs only), N-09 (dead code), N-81 (per-prompt reminder), N-94 (ripple check on every edit).
- **Release scripts:** C-04 (self-hosted queue routing), C-17 (queue flake publication), C-37 (the 43-row local gate, off the release path), C-39 (remaining policy pins). The fast-row catalog (C-38) moves into the lane lint job.
- **Adds 0.**

</details>

<details open data-collapse>
<summary>4.15: facts are the single source of truth (move 2): 45 removed, 2 added</summary>

- **Facts added:** `Installed` (host, release, binary SHA), `Held` (subject, gate code, as-of SHA, owner, next action) and `Closed` as of a release SHA; one notice projection with the key `(subject, fact kind, as_of)`.
- **Adds 2 monitors:** the notice projection, and the deadline monitor ("expected fact absent by T"). The liveness rows (W-12, W-19, W-58 and their cause codes) stay.
- **Gates now bound to facts, not moving state (9):** G-21, G-34, G-39, G-54, G-55, G-65, G-68, G-69, G-70. The park side of G-55 becomes a plain state transition; review stays enforced once, before merge (G-104).
- **Watchdogs merged into four projections (24):** owed action (W-03 absorbs W-01, W-02, W-04, W-09, W-11); worker liveness (W-05 absorbs W-06, W-07, W-08, W-10); boot and death (W-12 absorbs W-20; W-19 absorbs W-21; W-37 absorbs W-23); message delivery (W-27 absorbs W-28, W-29; W-31 absorbs W-32, W-33; W-34 absorbs W-35, W-69); verification expiry (W-39 absorbs W-38); merged-not-closed (W-45 absorbs W-46, W-47); W-48 into N-24; W-49 into W-42; W-54 into W-55.
- **Notices merged (12):** N-01, N-04, N-05, N-100 into the owed-action projection; N-12, N-13 into N-10; N-15, N-18 into N-14; N-36 rendered at read time instead of re-appended to every note; N-60 into N-59; N-62 into N-64; N-101 into N-06.
- **Ends:** the stall-page family (I-05–I-07), the transcript-mirror flood (I-16), the double wake on an expired dispatch (cas-cdab), and an installer hang going unnoticed for 9 h 30 min (I-15) instead of 20 min.

</details>

<details open data-collapse>
<summary>4.16: capability guards (move 3): 19 removed, 2 added</summary>

Prerequisites, all reversible: the OS sandbox on for Claude workers (writable roots = worktree, artifact dir, scratch); a macOS backend for the existing credential masking (per-worker `HOME` or a Seatbelt profile); a worker GitHub identity limited by ruleset to `factory/*`; a cargo shim on the worker `PATH` that calls the existing admission runner; and the fast rows as a merge-queue check, because today they run only at the worker pre-push.

- **Removes 19 guards:** H-29 and H-28 (workspace write roots), H-14 (config file writes), H-33 (protected patterns), H-09 and H-10 (operator credentials and denied CLIs), H-22, H-23 and H-25 (commit, local-merge push and raw `gh pr merge`), H-15 (push hook bypass), H-24 and H-26 (merge holds, becoming one required status check), H-03 and H-04 (Rust build text match, replaced by the cargo shim), H-05 (formatter spill, replaced by a one-time format normalization), H-08 (browser tier, enforced by the runner), H-12 (role env tampering, replaced by server-issued role), H-16 (process kill, replaced by PID or user isolation), H-17 (Neon production write, replaced by a branch-scoped credential).
- **Adds 2:** the sandbox profile and the server-side ruleset. H-02 stays, now fed by the shim instead of text matching. Per decision 3, a harness whose sandbox is not verified keeps its text guards until it is.
- **Ends:** all 21 shell-text false positives of the last 30 days, and the `pre_tool.rs` bypass/false-positive cycle.

</details>

<details open data-collapse>
<summary>4.17: one lane job; the queue is the only proof: 7 removed</summary>

- **Removes 7:** C-03, C-05 and C-06 merge into one hosted lane job (C-02); C-07 (pre-push fast rows, after 4.16 puts them in the queue); C-08 and C-35 (train full-set receipts: the queue is the proof); C-33 (cron watchdogs, once lanes are hosted).
- **Moves:** the CI-tiers script tests (C-10) to a non-required job that runs only when `scripts/` or `.github/` change.
- **Ends:** about 1,639 wall-minutes of train-branch full-set runs per 8 releases, and the five post-merge re-runs on a tree the queue had just proven.

</details>

**After 4.17 the budget applies.** The registry count (274) is the ceiling. Each release cut reports, per gate code, true catches, overrides and seconds spent; a gate with no true catch in three releases is listed for deletion.

## Success metrics, all measurable from data that exists today

| Metric | Target | Baseline |
| --- | --- | --- |
| commit→main: epic PR opened → merged | ≤ 30 min | last 8 releases: median 19.8 min (17.5–24.9), 8 of 8 on target |
| main→published: merged → GitHub release | ≤ 20 min | last 8: median 8.0 min (3.4–28.0), 7 of 8 on target |
| Cassy defects filed per release | down across 3 releases | v4.5.0–v4.12.0: 101 issues (median 10.5), 186 bug tasks (median 14) |
| Guard and gate false positives | down; zero per shell-text guard once replaced | Nov 9–10: 36 reviewed-drop overrides; 23 guard false positives in 30 days |
| Notices answered with no action | toward zero | Nov 9–10: 98 blocker wakes, 90 "Ignored." |
| Supervisor steps to close a shipped delivery | 0 | gabber cas-d628: 5 steps, 50 min 35 s |
| Check count (registry rows) | non-increasing | 388 today; 274 after 4.17 |

<details open data-collapse>
<summary>Where each metric comes from today</summary>

- **commit→main and main→published:** `gh pr list --state merged --json createdAt,mergedAt` and `gh release list` publishedAt; release PR is the train's epic PR since v4.1.
- **Defects per release:** `gh issue list --state all` bucketed by release publish windows, plus `task_type='bug'` rows in the store.
- **False positives:** today, reviewed-drop overrides in task notes and guard false-positive GitHub issues; after move 4, override facts per gate code.
- **Notices with no action:** `prompt_queue` blocker rows to the supervisor against replies "Ignored.".
- **Supervisor steps:** task notes on shipped deliveries; after 4.13, the release receipt.
- **Check count:** this inventory today; the registry after 4.14.

</details>

Per rule 176 (6), success is three consecutive real releases measured on target, not closed tasks.

## Risks, and what must not be removed

**Keep, whatever the budget says:** content-loss detection, review integrity, the merge queue and shipped-asset checks, liveness, idempotency constraints and build admission.

<details open data-collapse>
<summary>What stays, and why</summary>

- the content-loss check itself (it made a true catch of a real deletion in I-20); only its moving-target comparison moves into the reconciler;
- independent-reviewer lineage, artifact byte binding and typed verdicts;
- the merge queue as the only gate to `main`, the ISA audit and the launch smoke on shipped assets (cas-a677 §10.4);
- liveness: worker-died, the `cas serve` parent watchdog and daemon-loop health;
- the active-dispatch unique index and other database idempotency constraints;
- build-capacity admission, which is a resource lock, not a policy guard.

</details>

<details open data-collapse>
<summary>Risks and mitigations</summary>

**Risks.**

| Risk | Mitigation |
| --- | --- |
| A harness whose sandbox profile is unverified (Codex, Grok, OpenCode) loses its only protection when text guards go | Keep the text guards for that harness only, scoped and time-boxed, until its sandbox is verified (decision 3) |
| Codex's sandboxed mode refuses unattended MCP writes under `approval_policy=never`, which is why the light lane bypasses it (`cas-cli/src/light_lane.rs:47-51`) | Prove the Codex worker profile in a spike before deleting any guard that Codex workers rely on; until then Codex keeps its text guards (decision 3) |
| Mixed versions across prowl and soundwave read new fact kinds differently | Facts are versioned and additive; a reader that does not know a kind ignores it, and a view labels the peer "last known" |
| Collapsing watchdogs drops a signal no fact records | Each deleted watchdog names the fact or deadline that replaces it in the migration table; a watchdog without a named replacement stays |
| Generated docs are stale between cuts | Workers read source, not CODEMAP, to make changes; the session-start freshness notice goes away with the per-change regeneration |
| The registry becomes one more thing to maintain | The registry is generated from gate definitions in code (one macro per gate), not hand-written, and the release report reads it |

</details>

## Options

| Option | Cost | Risk | Effort | Reversibility | Outcome |
| --- | --- | --- | --- | --- | --- |
| **A. Four moves over five releases (recommended)** | 5 release epics, each net-negative in checks | medium: sandbox parity, mixed versions | medium | each release reversible on its own | 388 → 274 checks; close-after-merge gates 19 → 6; no shell-text guards on sandboxed workers |
| B. Ship 4.13 only and keep fixing incidents | lowest now | high: each incident adds a gate, as it did 3 times this week | low per incident, unbounded total | n/a | gate count keeps rising; content drops end, stall pages and guard false positives continue |
| C. Rewrite close and hook layers in one release | highest | high: one large change to the close path everything depends on | high | poor | same end state with no intermediate measurement |

**Why A.** It is the only option where every release removes more than it adds and is measured before the next one starts.

<details open data-collapse>
<summary>What we give up, and the reversal cost</summary>

**What we give up.** B is cheaper this week, and 4.13 alone removes the largest single cost (the 36 overrides). **Reversal cost.** Each step deletes code behind a fact or capability that already works; reverting one release restores its deleted gates from git without touching the others.

</details>

## Open decisions for the operator

1. **Approve the target and the release sequence.** Recommendation: approve; nothing is built before you do (rule 176).
2. **A worker GitHub identity.** Server-side push protection needs workers to push as someone other than you (today they push as `pippenz`). Recommendation: a GitHub App on Richards-LLC, limited by ruleset to `factory/*`; free and revocable in one click.
3. **Sandbox rollout order.** Every worker runs unsandboxed today. Recommendation: Claude workers first, Codex after a profile spike; text guards stay only for a harness whose sandbox is unverified, and no new text guards for any harness.
4. **The budget ceiling.** Recommendation: one total cap at 274, one in for one out, with a per-stage report at each cut. A per-stage cap would block a needed integrity gate.
5. **Generated docs.** Recommendation: generate CODEMAP and PRODUCT_OVERVIEW at release prep and compile journey bug families from per-task files, as CHANGELOG already is. Splitting journeys.yaml alone fixes one of three files.

## Evidence and provenance

<details open data-collapse>
<summary>Source pin, inputs, methods and data sources</summary>

- **Source pin.** Every `file:line` is at origin/main `01df2dc7768570578021fda791e2c79d444f1d77` (v4.12.0, merged 7:27 AM ET November 10), read from a `git archive` extract of that commit. The assigned worktree and epic base `9655aa0ae` (v3.96.0) is 2,917 commits behind main and was not used for citations.
- **Inputs read.** cas-a677's release-flow review (October 15, `cas-a229/release-flow-review/release-flow-review.md` on soundwave); the approved 4.13 design (`cas-47b2/close-at-release-design.md`); rule 176; `cas-cli/docs/CONTRIBUTING.md` build policy (line 1895 onward); the fact tables in `crates/cas-store/src/delivery_store.rs`, `verification_store.rs` and `qa_batch_store.rs`; worker spawn arguments in `crates/cas-pty/src/pty.rs`.
- **Inventories.** Five read-only research passes on November 10, 9:45–10:20 AM ET, wrote `inputs/inv-gates.md`, `inv-guards.md`, `inv-watchdogs-notices.md`, `inv-ci-release.md` and `incidents.md` under the task's artifact directory. Methods are in each file: grep of refusal headings and gate functions with production-only filtering; hook matcher and deny-marker scans; daemon and director job tables; workflow job enumeration; task notes, prompt-queue counts and GitHub issues. About 30 watchdog rows were written from source directly; hook, director and CI line numbers come from sub-scans, spot-checked.
- **Plan counts.** Computed by `build/plan_counts.py` from the five tables and the release assignment in `inputs/plan.json`; "reads moving state" uses each table's state column (G: `MOVING`; H: `SHELL-TEXT`; W and N: `RE-POLL` and its variants).
- **GitHub data.** `gh release list`, `gh pr list --state merged`, `gh run list` for `release.yml`, `full-validation.yml` and `release-prebuild.yml`, and `gh issue list --state all` on Richards-LLC/cassy, queried about 10:00 AM ET November 10.
- **Store data.** Counts of overrides, blocker wakes, dispatches and bug tasks come from prowl's `.cas/cas.db` opened read-only. Soundwave's store was not read, so the verification-dispatch flood stays unverified.
- **Estimates.** None of the counts above are estimates. The deadline-monitor timing for I-15 (20 min) is a design target, not a measurement.
- **Design voice.** `DESIGN.md@b43940e43`, inheriting `hub-web/DESIGN.md`. This is a design proposal; no product behavior described under the target architecture exists yet.

</details>

## Appendix: the full inventory and evidence

Every row is pinned to origin/main `01df2dc77` (v4.12.0), read from an extracted snapshot of that commit. Paths without a prefix are relative to the abbreviation keys given above each table. Verdicts are this design's proposal, not decisions.

<details open data-collapse>
<summary>A. All 127 close, park, merge and release gates</summary>

Paths without a prefix are under `cas-cli/src/mcp/tools/core/task/` (lifecycle files) or `cas-cli/src/mcp/tools/core/workflow/` (worktree_ops, train_merge, premerge_verification, release_*). Other paths are relative to `cas-cli/src/`, except `crates/...`.

Table: Gates G-01 to G-127

| ID | Gate | Stage | Source file:line @01df2dc77 | Defect it catches | State it polices | Duplicates | Origin | Verdict |
|---|---|---|---|---|---|---|---|---|
| G-01 | Explicit `requires_start` prerequisite still open | start | cas-cli/src/mcp/tools/core/task.rs:403 (call lifecycle.rs:2560) | Starting work whose declared prerequisite is unfinished | MOVING (current task rows) | G-21 (close-time blocks) | d7ff53adc (GH #886) | KEEP: operator-declared ordering, one store read |
| G-02 | External (cross-project) blocking dependency | start | task.rs:422 (call lifecycle.rs:2561) | Start while a cross-project proposal still blocks | MOVING (external dependency store) | none | ? | KEEP: real external ordering |
| G-03 | Origin project mismatch | start | task.rs:279 (call lifecycle.rs:2466) | Executing another project's task row here | MOVING (current task row) | none | ? | KEEP: wrong task identity |
| G-04 | Terminal task cannot start | start | lifecycle.rs:2480 | Restarting closed/cancelled work without reopen | MOVING (current task row) | G-30 | ? | KEEP: state-machine invariant |
| G-05 | Clean AwaitingMerge task cannot be restarted by worker | start | lifecycle.rs:2611; close_ops/gate_text.rs:311 | Worker reopening a parked, reviewed delivery | MOVING (current task row) | G-60 | cas-a844 / cas-5054 | KEEP: protects the parked SHA |
| G-06 | Release freeze / epic design-review staffing pause | start | epic_design_review.rs:292 -> workflow/release_freeze.rs:190 | Staffing a frozen train or paused epic | MOVING (freeze/pause state) | G-100 | f33af55ff (cas-2f4d) | KEEP: release policy, not a close gate |
| G-07 | Epic split hold (supervisor-granted start) | start | lifecycle.rs:2640-2660 | Taking a child promised to a peer machine | MOVING (live Cloud split hold) | G-15 | ? | KEEP: cross-machine ownership |
| G-08 | Single active WIP per worker | start | lifecycle.rs:2448 | Worker juggling two deliveries on one lane | MOVING (leases) | G-10 | ? | KEEP: lane hygiene |
| G-09 | Queued-start consistency (fresh context, snapshot changed, declared target) | start | lifecycle.rs:2412, 2549, 2576, 2581 | Queued successor starting on a stale context/row | MOVING (queue + task row) | none | ? | KEEP: race guard |
| G-10 | Worker lane identity (not worker's branch; foreign per-task branch; occupied lane; parked lane must stay; dirty reused branch; unresolved base) | start | lifecycle.rs:2527, 202/2793/2839, 134/2910, 490, 365/372, 382 | Starting on a branch that carries another task's work | MOVING (current branch tip) | G-60, G-91 | cas-dd0e, cas-7ce6 | KEEP: wrong task identity |
| G-11 | Worker checkout belongs to another repository | start | lifecycle.rs:2716 | Work landing in the wrong repo | MOVING (checkout) | G-40, G-102 | ? | KEEP: wrong repo identity |
| G-12 | Assignee mismatch (no self-dispatch) | start | lifecycle.rs:2993 | Worker taking an unassigned task | MOVING (task row) | none | ? | KEEP: assignment authority |
| G-13 | TASK START DEFERRED: unread supervisor messages | start | lifecycle.rs:3083 | Starting before reading corrections | MOVING (prompt queue) | none | ? | KEEP: cheap, no side effects |
| G-14 | Supervisors cannot start non-epic tasks | start | lifecycle.rs:3109 | Supervisor doing worker work | MOVING (role) | none | ? | KEEP: role split |
| G-15 | HELD BY A PEER (Cloud claim) | start | lifecycle.rs:749, 3266 | Two machines working one task | MOVING (live Cloud) | G-07, G-16 | c17a36a9f (cas-5f28) | KEEP: cross-machine ownership |
| G-16 | Local task lease lock / lease refused | start | lifecycle.rs:3349, 3363 | Concurrent lease holder | MOVING (lease table) | G-15 | ? | KEEP: mutual exclusion |
| G-17 | QA PREFLIGHT BLOCKED (QA round claim at start) | start | lifecycle/qa_dispatch.rs:375 (call lifecycle.rs:2536) | Starting a QA task whose round cannot be claimed | MOVING (qa_pass row) | none | ? | KEEP: reviewer identity |
| G-18 | Lifecycle outbox push failure on start | start | lifecycle.rs:3737; lifecycle/supervisor_push.rs:690 | Transition not durably recorded for supervisor | MOVING | G-85 | cas-062d | KEEP: durability |
| G-19 | Caller note-marker injection rejection | park | task.rs:19 (calls close_ops.rs:7717; worktree_ops.rs:4431; qa_pass_ops.rs:102) | Forged trusted audit markers in reason/summary | IMMUTABLE (request text) | none | ? | KEEP: audit integrity |
| G-20 | OUTCOME UNMET (phase outcome) | park | phase_outcome.rs:359 (call close_ops.rs:7735) | Closing dependent phase work before its measured outcome | MOVING (dependency outcomes) | G-21 | 5fd9439e1 | KEEP: measured product outcome, not delivery mechanics |
| G-21 | Open `blocks` dependency at close (shipped-blocker exemption) | park | close_ops.rs:7766-7786; qa_batch.rs:139 | Closing ahead of an unfinished blocker | MOVING (task rows + target ref) | G-01, G-20 | d7ff53adc / cas-4b22 | MERGE(into reconciler): close in dependency order against the shipped SHA |
| G-22 | supervisor_override authority + non-empty reason | park | close_ops.rs:4947 (call 7790) | Override by a non-supervisor or without reason | MOVING (agent registry) | G-108 override, G-126 | 849a49f38 (cas-6027) | KEEP: credential/authority |
| G-23 | Close-disposition exclusivity (no_delivery / evidence_only / negative_result / completion / external / Gate) | park | close_ops.rs:7804-7858 | Contradictory dispositions in one request | IMMUTABLE (request) | none | cas-6c50 | KEEP: parameter validation |
| G-24 | Non-integration close authority and receipt fields (evidence_only, negative_result) | park | close_ops.rs:5561-5631 | Non-supervisor declaring a no-merge outcome | MOVING (agent registry) | G-22 | 53ad7080b (cas-e205), cas-6c50 | KEEP: authority |
| G-25 | Evidence-only measurement (never-merge proof, workflow paths only) | park | close_ops/evidence_only.rs:45 (call close_ops.rs:8842) | Product code hidden in an evidence-only close | MOVING (target ref) | G-32 | cas-e205 | KEEP: hidden content |
| G-26 | GATE CLOSE REJECTED: Gate task needs DECISION note | park | close_ops.rs:5539, 5551 | Closing a decision gate with no decision | IMMUTABLE (notes) | none | ? | KEEP |
| G-27 | EXTERNAL VERIFICATION RECEIPT REJECTED (18 checks) | park | close_ops.rs:5647-5772 | External delivery claimed without a completed passing receipt for this task/epic/session | IMMUTABLE (delegation receipt) | none | 704a5f275 (cas-3924) | KEEP: independent verifier |
| G-28 | DELIVERY RECEIPT REJECTED (worker completion receipt: lease, identity, shape, tip, base) | park | close_ops.rs:5804-6149; proof_scope.rs:266 | Forged/stale receipt or wrong lease holder | IMMUTABLE (mints the receipt once) | G-60 | bf04899c5 | KEEP: creates the immutable delivery boundary |
| G-29 | DELIVERY PROOF SCOPE LOCKED | park | lifecycle/proof_scope.rs:256, 362 (call close_ops.rs:7892) | Editing proof scope after delivery locked it | IMMUTABLE | none | ? | KEEP |
| G-30 | Cancelled task cannot be closed | park | close_ops.rs:7914 | Overwriting cancellation history | MOVING (task row) | G-04 | ? | KEEP |
| G-31 | PROOF CLAIM RECEIPT REQUIRED | park | close_ops/proof_claims.rs:135 (call close_ops.rs:7937) | "Tests pass" with no run id/artifact | IMMUTABLE (reason text) | none | 286df5c70 (cas-b8e8) | KEEP: honest evidence |
| G-32 | No-code intent and proof (CODE PROOF REQUIRED, CODE INTENT VIOLATED/REJECTED, INLINE EXECUTION NOTE REJECTED) | park | close_ops.rs:369, 399, 425 (calls 7952, 11583, 16962) | Code shipped under a no-code label | MOVING (diff vs target) | G-25, G-82 | cas-099d, cas-525c | KEEP: hidden content |
| G-33 | TMPFS PROOF RECEIPT REJECTED | park | close_ops.rs:530, 7960-7975 | Proof on volatile storage | IMMUTABLE (reason text) | none | 55a128495 (cas-4060) | KEEP: durable evidence |
| G-34 | Exact verification dispatch must resolve (pending/claimed/unreadable; VERIFICATION TIMED OUT) | park | close_ops.rs:8009-8135; close_ops/gate_text.rs:87 | Closing while the bound review is in flight | IMMUTABLE (dispatch row) | G-68, G-104 | cas-5c33 | MERGE(into reconciler): read the verdict bound to the SHA instead of holding close on dispatch state |
| G-35 | Release verdict supersedes stale dispatch / shipped-timeout waiver | park | qa_batch.rs:590; close_ops.rs:7977-8007, 19970 | Timed-out Tier 1 dispatch wedging shipped work | IMMUTABLE (verdict + dispatch) | G-34, G-67 | cas-d8b2, cas-4b22 | DELETE: escape hatch obsolete once the reconciler binds verdicts to the release SHA |
| G-36 | WORK HALTED (urgent stop) | park | close_ops.rs:8148-8210; stale_close_guard.rs:72 | Worker closing past an operator stop | MOVING (agent metadata) | verification_tools.rs:634 | cas-b269 / cas-85fd | KEEP: operator control |
| G-37 | Epic verification owner gate | epic-close | close_ops.rs:53 (call 8239); lifecycle.rs:11-19 | Silent cross-session epic close | MOVING (session identity) | none | cas-9fff | KEEP: identity |
| G-38 | Close repo/target resolution fail-closed (repo context, CLOSE GATE GIT REPOSITORY ERROR, DEFAULT BRANCH ERROR) | park | close_ops.rs:8315, 8328, 18502/18527, 18543/18575 | Gates silently querying a non-repository | MOVING (config) | G-102 | cas-ede8 | KEEP: wrong repo identity |
| G-39 | PUSHED TASK RECEIPT REJECTED (retired implementer recovery) | close-after-merge | close_ops.rs:24961/24970 (call 8405) | Recovering a wrong or unpushed lane after request_changes | MOVING (origin branch) | G-28 | c33ecd9e9 (cas-9ffa) | MERGE(into reconciler): close from the shipped commit, no lane recovery |
| G-40 | Worker worktree resolution (CLOSE HOOK CONTEXT REJECTED, 14 sites) | park | close_ops.rs:12638/12683, 19180/19191 | Gates reading the wrong checkout | MOVING (checkout) | G-11 | cas-d220 | KEEP: binds park to task-owned checkout |
| G-41 | Declared pre-close hook (PRE-CLOSE HOOK FAILED; unreachable / target-only lint / stale rebased anchor) | park | close_ops.rs:25047 (calls 8429, 10832), 25022, 24771, 24872 | Project-declared executable check failing | MOVING (runs at current tip) | lane CI (project hooks often mirror CI) | c0dc7375c (cas-d220) | KEEP: project contract; run once on the anchor |
| G-42 | Task commit identity resolution | park | close_ops.rs:13491 (call 8467) | Unresolvable task-owned commits | MOVING (refs + lease history) | G-10 | cas-e74c | KEEP: identity |
| G-43 | commit_receipt repo binding (RECEIPT NOT FOUND IN THIS REPOSITORY) | park | close_ops.rs:17954/17975 (call 8877) | Foreign-repo SHA recorded as proof | IMMUTABLE (SHA) | G-58 | cas-fdc9 | KEEP: false-assurance guard |
| G-44 | INTEGRATION BATCH RECEIPT REJECTED (receipt must be the recorded squash) | close-after-merge | close_ops.rs:8846-8866 | Arbitrary SHA accepted as batch receipt | IMMUTABLE (batch.delivered_head) | G-56 | 1edccd9b3 | MERGE(into reconciler): release squash supersession recognised once |
| G-45 | Parked anchor resolution (PARKED DELIVERY UNRESOLVED; own-commit check) | park | close_ops.rs:18220/18243, 8917-8990 | Close measuring the next task's tip | IMMUTABLE (recorded anchor) | G-60, G-96 | cas-ba4a, GH #1177 | KEEP: identity |
| G-46 | Stale/prior commit_receipt at fresh park | park | close_ops.rs:7106/7146 (call 8992) | Parking an old or target-side head | MOVING (branch tip) | G-60 | d6cf3490d | KEEP: identity |
| G-47 | NO DELIVERY CLOSE REJECTED (declaration + absence measurement) | park | close_ops.rs:9001-9040 | Helper closing while delivery/QA exists | MOVING (refs + QA rows) | G-82 | 753dc7a49 (cas-a94f) | KEEP: audited exemption |
| G-48 | RULE REFERENCES REQUIRED | park | close_ops/rule_references.rs:8, 159 (calls close_ops.rs:9125, 6055) | Rule site without adjacent marker | IMMUTABLE (tip diff) | none in CI today | ff9ff2669 (cas-4f87) | MERGE(into lane CI fast row): deterministic diff lint |
| G-49 | Registry upkeep (JOURNEY REGISTRY BROKEN, BASE/DELIVERED REGISTRY INVALID, UNOWNED USER-FACING FILE, INTEGRATION PARENT MISMATCH) | park | close_ops/registry_upkeep.rs:11, 119, 206, 392, 566, 729 (call close_ops.rs:9130) | User-facing file without a journey owner | MOVING (base registry at current target) | merge-queue journeys | 87a1c7c2e (cas-e8d2) | MERGE(into lane CI fast row) |
| G-50 | Docs upkeep (CODEMAP DRIFT, PRODUCT_OVERVIEW DRIFT) | park | close_ops/docs_upkeep.rs:9, 138, 146 (call close_ops.rs:9138) | Stale navigation docs | MOVING (target docs) | ci.yml instruction-file check (partial) | 37f31e228 | MERGE(into lane CI fast row) |
| G-51 | REPORT NOT READY FOR THE OPERATOR (G7 report quality) | park | close_ops/report_quality.rs:13, 64 (call close_ops.rs:9148) | Operator report missing brief/critique/brand | IMMUTABLE (report artifact) | none | 8409bd6fe | KEEP: deliverable integrity |
| G-52 | QA LEDGER REJECTED + Tier 2 adversarial probe | park | lifecycle/qa_evidence_gate.rs:213, 263, 281/290 (calls close_ops.rs:9182, 10315) | Malformed ledger / Tier 2 delivery without probe | IMMUTABLE (ledger bytes) | G-118 | 752f8f771 | KEEP: walk evidence integrity |
| G-53 | QA EVIDENCE REJECTED (implementer evidence bundle, pair coverage, batch deferral) | park | qa_evidence_gate.rs:513, 722; live target 339 (call close_ops.rs:9216) | User-facing change without evidence bundle | MOVING (live target ref) | G-64, G-83 | cd69fc074 (cas-6ebf), cas-0cd5 | MERGE(into reconciler): batch path already defers to the release walk |
| G-54 | Fast-lane post-merge receipt (RECEIPT PENDING/FAILED/UNREADABLE, REVERT NEEDED, WALK FAILED) | close-after-merge | close_ops/fast_lane_gate.rs:132-250 (call close_ops.rs:9197) | Hotfix merged without its post-merge walk | IMMUTABLE (receipt) | G-53 | 468427280 (cas-5ea7) | MERGE(into reconciler): post-merge walk is a reconciler input |
| G-55 | Factory-branch merge-state gate (MERGE REQUIRED handoff; INVALID BRANCH NAME; DELIVERY BRANCH UNRESOLVED; FACTORY BRANCH MISSING; MERGE STATE UNVERIFIABLE; landed-anchor receipt) | park | close_ops.rs:16896, 16921, 16991, 17103, 17162, 16863, 17681; close_ops/task_attribution.rs:60 | Unmerged commits closed as done | MOVING (branch tip + target ref) | G-81, G-124, G-125, merge queue | cas-95ce / cas-4b3f / cas-cf64 | MERGE(into reconciler): post-merge re-measure becomes shipped-SHA containment; park side is a plain state transition |
| G-56 | Delivery content presence (DELIVERY CONTENT DROPPED / UNVERIFIABLE / CHECK TIMED OUT / REVIEW REJECTED; supersession) | close-after-merge | close_ops.rs:15670, 15714, 15732, 15880, 15886, 15925, 15333; close_ops/delivery_evolution.rs:39 | Merged-then-lost delivery content | MOVING (current target ref) | G-106 (mark-merged content), G-44 | be54c16fa (cas-b278) | MERGE(into reconciler): keep the check, run once at the release SHA with fragment/generated-file supersession |
| G-57 | Lagging epic integration receipt | close-after-merge | close_ops.rs:20203 (call 17423) | Child receipt on epic lane but not trunk | MOVING (target refs) | G-55 | cas-da3f | MERGE(into reconciler) |
| G-58 | INVALID TASK COMMIT RECEIPT | park | close_ops.rs:20031, 20715/20739 | Receipt is not this task's work / outside window | IMMUTABLE (SHA; ancestry read live) | G-43 | 1f07a3c7d (cas-26bb) | KEEP: identity |
| G-59 | Unpushed delivery tip (CLOSE REFUSED at park) | park | close_ops.rs:16322/16328 (call 9352-9358) | Parking an anchor origin does not have | MOVING (origin ref) | G-107 PUSH FIRST | cas-44e93 | KEEP: park must bind a pushed anchor |
| G-60 | Anchor absent / unattributable advance / persisted anchor unreadable (PARK REFUSED x4) | park | close_ops.rs:9394-9453, 9546-9560, 6828 | Anchor moved onto another task's commits | MOVING (branch tip vs recorded anchor) | G-45, G-46, G-97 | cas-3d70, cas-ba4a, cas-00eb | KEEP: wrong task identity |
| G-61 | FOCUSED PROOF REQUIRED (crate test receipts at exact tip) + PROOF SCOPE INCOMPLETE (blast-radius modules) | park | close_ops/receipt_scope.rs:136, 160 (call close_ops.rs:9472) | Rust change with no receipt in its crate | IMMUTABLE (receipt at tip) | merge-queue Rust suite | cas-eac2, cas-d6eb | KEEP: only per-delivery Rust proof before the train; reads receipts, runs nothing |
| G-62 | CHANGELOG [Unreleased] / fragment required | park | close_ops/receipt_scope.rs:233, 267 | Shipped source with no changelog entry | IMMUTABLE (tip diff) | ci.yml release-note draft lint (partial) | cas-a945 | KEEP: at park only; reconciler must treat release-folded fragments as superseded |
| G-63 | TEST_ASSERTION_PROSE (test-shape checker at park) | park | close_ops/receipt_scope.rs:169, 198 | Tests asserting human messages | IMMUTABLE (tip diff) | scripts/release-gate.sh:378 fast row (pre-push hook + lane CI) | ? | DELETE: duplicates the fast row |
| G-64 | INDEPENDENT QA REQUIRED close gate (passed/waived QA at landed tip) | close-after-merge | lifecycle/qa_dispatch.rs:1275, 1360-1573 (call close_ops.rs:9612) | User-facing merge without passed QA at the landed tip | MOVING (target ref) | G-94, G-98 | dc7b8ab58 (cas-619f) | MERGE(into reconciler) |
| G-65 | Exact approved SHA must land (verified delivery) | close-after-merge | close_ops/verified_delivery.rs:267 (call close_ops.rs:9599) | Earlier anchor merged, approved correction did not | MOVING (target ref) | G-55 | ? | MERGE(into reconciler) |
| G-66 | Rapid-response migration review | park | rapid_response.rs:342 (call close_ops.rs:9755) | Migration in an incident window without human review | MOVING (window) | none | cas-1b4d | KEEP: human review |
| G-67 | WAITING FOR RELEASE VERDICT | close-after-merge | close_ops/gate_text.rs:333; close_ops.rs:9800-9843; qa_batch.rs:577; review_tier.rs:145 | Tier 1 closing before release review | IMMUTABLE (verdict at tip) | G-34, G-83 | 553723c8a (cas-ce5e) | MERGE(into reconciler): this is the reconciler's own input |
| G-68 | Verification jail (VERIFICATION REQUIRED / FAILED / timeout; worker handoff) | park | close_ops.rs:9853-10735 (10017, 10070, 10173, 10516, 10697, 10703); gate_text.rs:339 | Closing without an approved verdict | IMMUTABLE (verdict bound to cycle) | G-34, G-104 | cas-5597, cas-8725 | MERGE(into reconciler): review is enforced pre-merge by G-104 |
| G-69 | Repository-proof revalidation after verdict | close-after-merge | lifecycle/repository_proof.rs:575, 751; close_ops.rs:9893-9950 | Files changed after approval | MOVING (worktree/target) | G-104 (candidate binding) | cas-5c33 | MERGE(into reconciler) |
| G-70 | PRE-MERGE APPROVAL MISSING / premerge promotion | close-after-merge | close_ops.rs:10328, 9890; premerge_verification.rs:868 | Merged factory delivery with no pre-merge verdict | IMMUTABLE (verdict) | G-104 | 9c14aca43 | MERGE(into reconciler) |
| G-71 | EPIC DELIVERY TIP REQUIRED / epic build basis | epic-close | close_ops.rs:2734/2757, 2556 (calls 11228, 11243) | Epic closed without a measurable tip | MOVING (epic branch / default branch) | G-80 | 10c7531d0 (cas-b36b) | MERGE(into reconciler) |
| G-72 | WORKTREE MERGE REQUIRED jail (System-A worktree) | epic-close | close_ops.rs:10738-10790 | Unmerged legacy worktree | MOVING (worktree store) | G-55, G-124 | 1b7889bea (initial release) | DELETE: legacy, fully duplicated |
| G-73 | UNCOMMITTED WORK in isolated worktree (+ stray main-checkout note) | park | close_ops.rs:13803, 10854-10958, 14112 | Uncommitted edits lost at park | MOVING (worktree) | pre-push hook (partial) | 6e6d6a4b9 (cas-895d) | KEEP: lost content |
| G-74 | Execution-note diff contracts (ADDITIVE-ONLY / VALUE-ONLY VIOLATION) | park | close_ops.rs:14271, 14291, 14309 (calls 11013, 11060) | Declared additive/value-only task modifies or deletes | IMMUTABLE (delivery diff) | none | 8d0fc191e (cas-e235), 1c31dd51f (cas-8ad8) | KEEP: cheap declared contract |
| G-75 | SNAPSHOT APPROVAL REQUIRED | park | close_ops/snapshot_approval.rs:39, 50 (calls close_ops.rs:11354, 6060) | Golden snapshot changed without approval | IMMUTABLE (range + notes) | none | 020fbac06 (cas-0c55) | KEEP: human approval |
| G-76 | Loaded proof for risk=concurrency | park | close_ops.rs:2875-2883; close_ops/loaded_proof.rs:72, 86 | Concurrency change without a load run | IMMUTABLE (notes) | none | ? | KEEP: not covered by CI |
| G-77 | Blast-radius proof_targets narrower than diff | park | close_ops.rs:2885-2896 | Declared proof narrower than the diff | MOVING (diff vs attributed base) | G-61 PROOF SCOPE INCOMPLETE | cas-b38a | MERGE(into G-61) |
| G-78 | Platform proof + PLATFORM RISK LOOKS IMPLAUSIBLE | park | close_ops.rs:2907-2930, 2444/2462 | macOS breakage | IMMUTABLE (notes) | .github/workflows/ci.yml:2255 macOS Check on PRs to main | cas-90e8, 7fa22580b (cas-b4cc) | DELETE: macOS Check runs on the release PR |
| G-79 | Scoped integration-test proof (SCOPED_PROOF targets/base, SURFACE UNRESOLVED, INCOMPLETE args, inherited base-red, merge-queue alternative) | park | close_ops.rs:2936-2990, 1683, 1711, 1780, 1817, 2171; close_ops/merge_queue_proof.rs:44, 101 | Integration targets not run | MOVING (base from attribution window) | G-61, merge-queue suite | cas-0db7, cas-a9a1, cas-74cb | DELETE: merge queue plus focused receipts cover it; base recompute is a recurring bounce source |
| G-80 | BUILD PROOF / ASSEMBLY_PROOF | epic-close | close_ops.rs:2527, 2792 (call 11406), 11392-11440 | Epic tip never built | MOVING (epic tip) | merge-queue full build | cas-4cbb, cas-3efd | DELETE: merge queue proves the final tree |
| G-81 | MERGE REALITY CHECK (B2: 0 commits, never pushed) | park | close_ops.rs:18339, 18436, 18451 (call 11632) | Worker committed elsewhere | MOVING (refs) | G-55, G-59, G-45 | 05ea53a62 (cas-762e) | DELETE: anchor identity and unpushed-tip checks cover it |
| G-82 | Zero-commit ambiguity (CODE TASK DIFF/COMMIT CLOSE) | park | close_ops.rs:20857, 20916, 20970 (call 11658) | Code task closed with no delivery | MOVING (refs) | G-47, G-32 | cas-ee2b | MERGE(into G-47) |
| G-83 | Deferred batch QA debt ("waiting for batch receipt") | close-after-merge | qa_batch.rs:800, 808 (call close_ops.rs:11695) | Batch-deferred delivery closing without a receipt | IMMUTABLE (receipt lists anchor) | G-53, G-67 | c58b9ac24 | MERGE(into reconciler) |
| G-84 | Final-write compare-and-swap (CLOSE REFUSED: changed while running) | close-after-merge | close_ops.rs:11971-12035 | Concurrent reopen/proof_scope_fix overwritten | MOVING (row clock) | none | cddb9edf0 (cas-c202) | KEEP: race safety |
| G-85 | Closed outbox push failure | close-after-merge | close_ops.rs:12157 | Close not durably pushed | MOVING | G-18 | cas-062d / cas-17e4 | KEEP: durability |
| G-86 | Direct `update status=closed` guards (DELIVERY CLOSE BLOCKED / CHECK FAILED; merge predicate) | close-after-merge | close_ops.rs:7448/7455/7470, 7510, 7519 | Bypassing the delivery state machine | MOVING (transaction + refs) | G-55 | ? | KEEP: closes the bypass |
| G-87 | TASK CANCEL REJECTED (supervisor authority) | close-after-merge | close_ops.rs:5785/5789, 12836 | Non-supervisor cancelling or reopening cancelled work | MOVING (agent registry) | G-22 | ? | KEEP: authority |
| G-88 | request_changes role gate | close-after-merge | close_ops.rs:4932 | Non-reviewer returning a delivery | MOVING (role) | none | ? | KEEP: reviewer identity |
| G-89 | Release receipt close admission (CI_OVERRIDE / REQUEST_CHANGES / NOT_PARKED) | close-after-merge | lifecycle/merge_close.rs:17-34; cli/release_automation.rs:484 | Shipped task not parked cannot be release-closed | MOVING (task row status) | G-90 | 884480679 (cas-e8cc) | MERGE(into reconciler): close against the shipped commit regardless of park state; CI_OVERRIDE stays a recorded exception |
| G-90 | Release-covered close re-runs ordinary gates; restores verdict hold (Held / TASK_STORE_UNREADABLE) | close-after-merge | merge_close.rs:626, 636, 795, 878 | Holds shipped deliveries on live-state gates | MOVING (re-runs G-21..G-83) | G-21 to G-83 | ? | MERGE(into reconciler) |
| G-91 | Pushed delivery tip ownership (merge-on-green close) | close-after-merge | merge_close.rs:47, 82 | Supervisor-authored commit counted as worker delivery | MOVING (origin + local refs) | G-10 | cas-46ea, cas-e9d6 | KEEP: identity |
| G-92 | Project hold | merge-admission | crates/cas-store/src/project_hold.rs:54; worktree_ops.rs:4425 | Merging during an operator hold | MOVING | none | ? | KEEP: operator control |
| G-93 | Foreign factory-session owner | merge-admission | factory_session_scope.rs:225; worktree_ops.rs:4468, 4535; train_merge.rs:263 | Integrating another session's branch | MOVING (live sessions) | none | cas-bebc | KEEP: identity |
| G-94 | Pending/unreviewed QA round on branch | merge-admission | qa_pass.rs:760, 767; worktree_ops.rs:4500, 4763, 4956 | Merging during an open QA round | IMMUTABLE (round bound to tip) | G-98, G-64 | ? | KEEP: independent walk |
| G-95 | Unanswered peer ask | merge-admission | peer_ask_gate.rs:107; worktree_ops.rs:4324, 4552 | Merging over own open question | MOVING | none | cas-bddf9 | KEEP |
| G-96 | Parked-delivery binding (AMBIGUOUS / INCOMPLETE / BRANCH UNRESOLVED / NOT PARKED HERE / REMOTE DELIVERY REFUSED) | merge-admission | worktree_ops.rs:3834, 3859, 3872, 3914, 3953, 4101, 4140 | Stale lane treated as a delivery | IMMUTABLE (anchor) | G-45 | ? | KEEP: identity |
| G-97 | Parked source drift (SOURCE CHANGED / SOURCE MOVED / CI RETRY SOURCE MOVED; stacked-lane source) | merge-admission | worktree_ops.rs:4728, 5643, 5155, 2073, 2088, 2268 | Merging a tip other than the reviewed anchor | IMMUTABLE (anchor) | G-60 | 570babaf9, cas-7ce6 | KEEP: identity |
| G-98 | INDEPENDENT QA REQUIRED before merge | merge-admission | qa_dispatch.rs:1214, 1238; worktree_ops.rs:4749 | User-facing merge without a walk | IMMUTABLE (round at tip) | G-94, G-64 | dc7b8ab58 (cas-619f) | MERGE(into G-94) |
| G-99 | RELEASE TRAIN OPEN (trunk merge refused) | merge-admission | worktree_ops.rs:4867-4905 | Trunk advanced past an open train | MOVING (train rows) | none | ? | KEEP: one integration per release |
| G-100 | RELEASE TRAIN CUT / TRAIN FROZEN (merge) | merge-admission | task/release_train.rs:150/160; workflow/release_freeze.rs:136, 165; factory_ci_retry.rs:40 | Merging into a frozen or cut train | MOVING (freeze state) | G-06 | 21fc47fca (cas-2b8d), f33af55ff (cas-2f4d) | KEEP: freezes the release SHA the reconciler keys on |
| G-101 | TRAIN MERGE ACTIVE reservation | merge-admission | workflow/train_merge.rs:94-120; worktree_ops.rs:4860, 5092 | Single merge racing a train merge | MOVING (queue file) | none | 2963ebe62 | KEEP: serialization |
| G-102 | WORKTREE REPOSITORY MISMATCH / worktree not found | merge-admission | worktree_ops.rs:4921, 5000, 5313 | Merge into the wrong repo | MOVING | G-38 | ? | KEEP |
| G-103 | Queued train conflict preflight / train target diverged | train-merge | worktree_ops.rs:5011-5035 | Queue stranded on a known conflict | MOVING (target ref) | G-110 | ? | KEEP: returns conflicted delivery early |
| G-104 | PRE-MERGE VERIFICATION hold (needs approval / timed out / drifted / TARGET_DIVERGED_FROM_ORIGIN / candidate manifest) | merge-admission | workflow/premerge_verification.rs:285, 296, 1010, 1122, 1134, 1140, 1208, 1213, 1413, 1428; worktree_ops.rs:5060 | Merging without a verdict on the candidate | IMMUTABLE (candidate SHA; verdict rebind at :1289) | G-68, G-70 | 35238c762 | KEEP: independent reviewer at the right point |
| G-105 | Fresh target containment unverified | merge-admission | worktree_ops.rs:5110-5135 | Merging on a stale containment assumption | MOVING (target ref) | G-110 | ? | KEEP |
| G-106 | TRANSACTIONAL DELIVERY state machine + target CAS (tip_changed) + mark-merged content proof | merge-admission | worktree_ops.rs:5191, 5201, 5576, 5875, 6045, 5961-5990 | Target moved mid-merge; content absent at mark-merged | IMMUTABLE (receipt) + CAS | G-56 | bf04899c5, cas-0a21 | KEEP: lost content / race |
| G-107 | PUSH FIRST (missing remote commit) | merge-admission | worktree_ops.rs:87, 97 | Unpublished source | MOVING (origin) | G-59 | 570babaf9 | KEEP |
| G-108 | Lane CI admission (CODE CI REQUIRED / CI RED / CI PENDING; override authority + decision log) | merge-admission | worktree_ops.rs:105-130, 5378-5500 | Merging red, pending or unvalidated code | MOVING (live GitHub checks at fixed SHA) | lane CI itself | 19b1a3463 (cas-a9bd), e2574ee70 (cas-6c13) | KEEP: enforces CI rather than duplicating it |
| G-109 | Fast-lane eligibility at merge (needs pre-merge PASS) | merge-admission | worktree_ops.rs:5342-5360 | Hotfix bypass without review | IMMUTABLE (eligibility at head) | G-54 | cas-5ea7 | KEEP |
| G-110 | Target prepare / TARGET DIVERGED from origin | merge-admission | worktree_ops.rs:5592-5620 | Merging onto a diverged target | MOVING (target ref) | G-103 | cas-42e1 | KEEP |
| G-111 | PROTECTED_DEFAULT_BRANCH_REQUIRES_PR / PR source moved | merge-admission | worktree_ops.rs:5708-5770 | Direct push to protected trunk | MOVING (live GitHub) | GitHub branch protection | cas-662e | KEEP: credential/rules |
| G-112 | Publication confirmation (NOT PUSHED / PUSH NOT CONFIRMED) | merge-admission | worktree_ops.rs:1453, 1476, 6109, 6296 | Local-only merge reported as published | MOVING (origin) | none | cas-5ee0 | KEEP: lost content |
| G-113 | train_merge preconditions (supervisor, session, open epic, epic/ target, queue ownership, ordered list, parked, same train, same repo, owner live) | train-merge | workflow/train_merge.rs:137-300, 353 | Wrong train, target, owner or unparked entry | MOVING | G-93 | 2963ebe62 | KEEP: identity/serialization |
| G-114 | TRAIN SUITE RED hold (+ override) | train-merge | train_ci.rs:20-90; train_merge.rs:362 | Merging onto a red train | IMMUTABLE (red receipt at tested head) | merge queue | 1267c042c (cas-f0a9) | KEEP: enforces train CI |
| G-115 | Train entry awaiting dispatch / returned entry skip | train-merge | train_merge.rs:385-400 | Merging an entry with unresolved review | IMMUTABLE (entry SHA) | G-104 | ? | MERGE(into G-104) |
| G-116 | batch_receipt / release verdict validation (qa.mode=batch, full staging SHA, ledger check, report names each task, project binding, implementer exists, no self-review, author independence, anchor ancestor of staging, staging on target) | release-verdict | service/qa_pass_ops.rs:14-60; qa_batch.rs:185-236, 311-371, 377-476 | Self-review, wrong project, anchor not in staging SHA, ledger mismatch | IMMUTABLE (staging SHA, ledger SHA-256) | none | c58b9ac24, 553723c8a | KEEP: reconciler input |
| G-117 | Release tag verdict (tag carries each delivery's tree effect, same across repos) | release-verdict | qa_batch.rs:498-553 | Tag lacking a delivery | IMMUTABLE (tag SHA) | none | ? | KEEP: becomes the reconciler key |
| G-118 | qa_record verdict validation (ledger, issue scope, no self-review, round evidence at tip, QA impact ledger refusal) | release-verdict | qa_pass_ops.rs:90-170; lifecycle/qa_impact.rs:184 | Self-review or evidence not at the tip | IMMUTABLE | G-52 | cas-c3b8 | KEEP: independent reviewer |
| G-119 | qa_waive authority and head binding | release-verdict | qa_pass_ops.rs:300-440 | Waiver on the wrong head | MOVING (origin tip) | none | cas-5c38, cas-6c75 | KEEP: authority |
| G-120 | POST-RELEASE CHECK / RELEASE NOTES BLOCKED (post-release watch) | epic-close | qa_post_release.rs:778 (call close_ops.rs:7745) | Epic closed before the release watch is clean | IMMUTABLE (receipt per release SHA) | none | 92c875e7e (cas-7465) | MERGE(into reconciler): finish hierarchy after the watch |
| G-121 | Release track checks (train branch, full SHA, commit == train tip, version owner, input audit stale) | release-verdict | cli/release_automation.rs:184-235 | Cutting from the wrong branch/commit | IMMUTABLE (commit) | none | ? | KEEP |
| G-122 | Auto-cut preconditions (setup declined, owner absent, no fragments, queue busy, cadence, notes draft) | release-verdict | workflow/release_auto_cut.rs:124-190 | Premature or orphan cut | MOVING | none | ? | KEEP: release policy |
| G-123 | reconcile_shipped_train (tag published, contains lane; children via ordinary close; train stays open on any hold) | epic-close | cli/release_automation.rs:491-716, 718-731, 458 | Train closed while children unshipped | IMMUTABLE (tag) but delegates to MOVING gates via G-90 | G-90 | 884480679 (cas-e8cc) | MERGE(into reconciler): seed of the reconciler; drop ordinary-gate delegation |
| G-124 | Epic unmerged worker branches (namespace check) | epic-close | cas-cli/src/mcp/tools/mod.rs:203 (call close_ops.rs:8584-8600) | Epic closed with unmerged lanes | MOVING (branch refs) | G-125 | ? | MERGE(into reconciler) |
| G-125 | Per-child epic merge gate (EPIC CLOSE CHECK INCOMPLETE / EPIC CLOSE BLOCKED DO NOT BLIND-MERGE / MERGE REQUIRED) | epic-close | close_ops.rs:23233, 23287, 23493, 23502 (call 8635) | Stranded child commits | MOVING (child refs vs epic target) | G-55 | cas-8f8f, 3efb5544a (cas-b192) | MERGE(into reconciler) |
| G-126 | EPIC CLOSE OVERRIDE REJECTED (stranded_branch_override) | epic-close | close_ops.rs:22767, 8676-8700 | Improvised escape from G-125 | MOVING (agent registry) | G-22 | cas-b192 | DELETE: door for G-125; obsolete once G-125 merges |
| G-127 | Gone epic branch fallback / child retarget | epic-close | close_ops/epic_autoclose.rs:151, 169, 184; close_ops.rs:8564-8606 | Deleted epic branch blocks close | MOVING (branch refs) | G-125 | cas-7f60 | MERGE(into reconciler) |

</details>

<details open data-collapse>
<summary>B. All 46 agent guards and the 25 shell recognizers</summary>

Abbreviations: `PT` = cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs, `SE` = …/pre_tool/shell_effects.rs, `OC` = …/operator_credentials.rs, `WDT` = …/worker_denied_tools.rs, `TEAMS` = cas-cli/src/ui/factory/daemon/runtime/teams.rs.

Table: Guards H-01 to H-46

| ID | Guard | Hook event / layer | Source file:line @01df2dc77 | What it denies; defect it prevents | Mechanism | Capability replacement | Known false positives | Verdict |
|---|---|---|---|---|---|---|---|---|
| H-01 | Codex native entry fail-closed | PreToolUse, Codex entry before telemetry | cas-cli/src/cli/hook/native_pre_tool.rs:27,30,44,57; cas-cli/src/main.rs:101-109 | Denies everything with "CASSY GUARD UNAVAILABLE" or "CASSY CARGO GUARD UNAVAILABLE" when hook input is invalid, the 2 s cargo-validation budget elapses, or processing fails. Prevents fail-open when the hook crashes. | infra (structured) | none, keep | none filed | KEEP. Fail-closed wrapper; shrinks as guards move out |
| H-02 | Worker-check cargo rewrite | PreToolUse, Bash, worker | PT:39-104 (route), PT:461-471 `worker_cargo_request`, PT:1764 `worker_check_command`, PT:1815 literal form, PT:50/53/68 denials (no root, PT:1882 log path, `&` without log); runner cas-cli/src/factory_worker_check.rs:268 `allowed_args`, 593 `acquire_slot` | Rewrites literal `cargo check -p X --lib/--tests` and `cargo nextest run -p X -E 'test(..)'` into `cas factory worker-check`. Admits it under max_concurrent_builders and PSI, and records receipts. Prevents host OOM and thrash from parallel worker builds. | SHELL-TEXT parse + rewrite | Cargo admission shim on the worker PATH (or `CARGO`/`RUSTC_WRAPPER` env) calling the same runner, plus a per-worker cgroup memory/CPU cap. Or an explicit MCP/CLI `check`/`test` tool, so no rewrite of free text is needed. | GH #1694 (env-prefix HOME rejected; fixed 68eecf4f0, d21393511) | MERGE. Keep the admission runner; replace text matching with a PATH shim or explicit tool |
| H-03 | NO WORKER RUST BUILDS | PreToolUse, Bash, worker | PT:625-639; PT:1923-1962 `worker_command_rust_build`; PT:1965 `CARGO_BUILD_SUBCOMMANDS`; PT:1993 `rust_build_invocation`; PT:2124-2235 Makefile rule parser | Denies any other cargo build/test/clippy, including through `make` targets whose Makefile recipe runs cargo. Prevents builds that skip the H-02 cap. | SHELL-TEXT parse (and parses Makefiles) | Same cargo shim plus cgroup cap. The shim sees every cargo invocation (make, scripts, `sh -c`), which the parser cannot. | none filed (shares H-02 surface) | DELETE (replace by cargo PATH shim + cgroup) |
| H-04 | Verifier Rust build | PreToolUse, Bash, agent_type=task-verifier | PT:645-653; PT:450-458 `verifier_rust_build_denial` | Same as H-03 for task-verifier subagents; sends them to `cas factory verifier-test`. | SHELL-TEXT parse | Same shim | none filed | MERGE into the H-02/H-03 shim |
| H-05 | Unscoped worker format run | PreToolUse, Bash, worker | PT:606-622; PT:2239 `worker_command_runs_dangerous_formatter`; PT:2245 `formatter_invocation_can_spill` | Denies mutating `cargo fmt` or rustfmt without `skip_children`. Prevents a workspace that is not rustfmt-clean from spilling hundreds of unrelated rewrites (cas-eb39, cas-852a0). | SHELL-TEXT parse | Root-cause fix: normalize the workspace format once and add rustfmt.toml. A pre-commit diff-scope check can also reject unrelated files. | none filed | DELETE (replace by a one-time fmt normalization) |
| H-06 | Slack transport (Violet only) | PreToolUse, all roles, before auto-approve | PT:485-489; cas-cli/src/hooks/handlers/handlers_events/slack_transport.rs:8,30,55; matcher cas-cli/src/config/hooks.rs:253-254 | Denies Slack writes not made through Violet (direct connector tools, `mcp_execute` dispatch to Slack). Operator transport policy. | structured tool-arg (plus `server.tool(...)` text in mcp_execute code) | harness tool config: do not install or allow Slack connector tools in factory settings; proxy allowlist | none filed | KEEP. Structured, cheap, no false positives |
| H-07 | Publication gate | PreToolUse, all roles | PT:492-496; cas-cli/src/hooks/handlers/handlers_events/publication_gate.rs:21, 329 | Denies `violet_post` of a file or deliverable while the epic still has open verification (GH #1057, a retracted PDF). | structured tool-arg + store | MCP-side check inside the Violet proxy dispatch or artifact post handler, where the post actually happens | none filed | MERGE (move into the post handler; server-side authority) |
| H-08 | Browser test tier | PreToolUse, Bash, factory non-supervisor | PT:498-500; cas-cli/src/hooks/handlers/handlers_events/browser_tier_guard.rs:120 `unfiltered`, 209 `denial` | Denies unfiltered Playwright, `journeys` and `journey-eval.sh --full` for workers. Prevents full browser suites on the shared host. | SHELL-TEXT parse | Runner-side enforcement: journey-eval.sh, run-verified-tests.mjs and the Playwright config refuse an unfiltered run when CAS_AGENT_ROLE=worker | GH #1366 (all targeted runs refused when journey-eval.sh missing) | DELETE (replace by a runner-side check) |
| H-09 | Worker operator-credential store read | PreToolUse, every tool, worker | PT:502-506 → WDT:89-97, 99-103 (message cites GH #1293) → OC:24 `references_store`, OC:95 `shell_references_store`, OC:158-160 heredoc bodies, OC:163-221 statement matcher (OC:189 recursive, OC:197 path check), OC:299 (`rg` and `find` always recursive), OC:525-547 `path_references_store` (**OC:540-541 ancestor match**), OC:575-598 glob reach, OC:780 `script_constructs_store_path`; store roots from crates/cas-pty/src/store_isolation.rs:29-34 | Denies any worker tool call or shell command that may reach the operator's Neon/Vercel CLI auth dirs. Prevents workers minting prod Neon owner credentials (GH #1293). | SHELL-TEXT parse (plus structured path fields for Read/Grep/Glob/Edit/Notebook tools) | **Per-worker HOME/credential scoping.** It already exists on Linux as a bwrap tmpfs mask (store_isolation.rs:1-18, pty.rs:2569). macOS has "no backend yet; the hook remains the only control" (store_isolation.rs:17-18). Use a per-worker HOME or a Seatbelt profile on macOS. | GH #1700, #1754; today about 9:43 AM ET (unfiled): `find` of the home dir; today 10:06 AM ET (this research): denied the heredoc that wrote this file, because the doc text names store paths | DELETE (replace by per-worker HOME/credential scoping; extend store_isolation to macOS). The module itself says parsing "cannot stop every reader" (store_isolation.rs:4-6) |
| H-10 | Worker-denied CLIs and MCP tools | PreToolUse, Bash and MCP, worker | PT:511-513; WDT:72-84 `denial`, WDT:131 `denied_cli` (via PT:5954 `shell_invocations`), WDT:150 package runners, WDT:162 `denied_mcp_tool`, WDT:186 mcp_execute dispatch parse | Denies `vercel`/`neonctl` (including via npx, bunx, pnpm dlx) and `*vercel*`/`*neon*` MCP tools. Prevents device-login prompts on the operator screen (GH #1219) and credential use. | SHELL-TEXT parse (CLI half) + structured (MCP half) | CLI half: worker PATH without these CLIs, and credential scoping (no creds, so a non-interactive fail). MCP half: proxy `supervisor:` routes and withheld env (H-43) already do this server-side. | none filed | MERGE (drop the CLI text half; MCP half moves to H-43) |
| H-11 | Operator-only policy commands | PreToolUse, Bash, all roles | PT:516-529; PT:6167 message (GH #1182); PT:6187 `operator_policy_command_denial`; PT:6255 clap parse; PT:6405 textual catch-all; PT:6476 `read_only_statement`; PT:6568-6989 reader allowlists (jq, awk, find) | Denies agents running `cas config set factory.write_roots` or `grant-write` (cas-3147, GH #1169). | SHELL-TEXT parse | Already enforced by the CLI: cas-cli/src/cli/config/operator_ops.rs:75-92 `require_operator` plus typed `confirm`, using `InvocationContext` (operator_policy.rs:963: env, ancestors, cgroup, TTY). An agent Bash has no TTY, so it cannot pass the typed confirmation. | commits 82b122b9f and 31d922649 (cas-b181: read-only Python text refused) | DELETE (H-41 already enforces) |
| H-12 | Role env tampering | PreToolUse, Bash, all roles | PT:526; PT:7101-7117 `role_env_tampering_*` | Denies unsetting or overriding CAS_AGENT_ROLE or CAS_FACTORY_MODE. Exists only because role identity is an env var. | SHELL-TEXT parse | Role identity from the MCP registered agent row or a server-issued token, not env; H-41 checks ancestors and cgroup as well | none filed | DELETE (replace by server-issued role identity) |
| H-13 | Supervisor-only MCP servers through the `cas mcp` CLI | PreToolUse, Bash | PT:532-544; PT:7041 `supervisor_only_mcp_command_denial`; PT:5942 `cas_invocations` | Denies agent `cas mcp add/remove/import` that would replace or alias a supervisor-only server (cas-1b94). | SHELL-TEXT parse | Already enforced by the CLI (cas-cli/src/cli/mcp_cmd.rs:227, 426 `guard_supervisor_only` with `InvocationContext`) and by the MCP side (H-44) | none filed | DELETE (duplicate of H-42 and H-44) |
| H-14 | Cassy config file direct write | PreToolUse, Write/Edit/Bash, all roles | PT:546-548; PT:5861-5940 `cas_config_file_write_denial` (Bash via PT:4732 `bash_write_targets`) | Denies writing .cas/config.toml, config.yaml or the global cas config directly (cas-0d4f0). | structured (file tools) + SHELL-TEXT (Bash) | Scoped FS sandbox: config paths read-only to agents | none filed | MERGE into the FS sandbox that replaces H-29 |
| H-15 | Worker push hook bypass | PreToolUse, Bash, factory non-supervisor | PT:553-569; PT:2348 `git_push_bypasses_required_hooks`; PT:2359 env bypass; PT:2528 hooksPath writes; PT:2567-2589 (push only) | Denies `git push --no-verify` or `-n`, `-c core.hooksPath=`, and GIT_CONFIG env overrides. Prevents skipping the worker pre-push hook (H-40). Note: `git commit --no-verify` is not covered (PT:2572-2574). | SHELL-TEXT parse | Server-side required status checks / ruleset. Caveat: the fast rows run only at pre-push, because lane CI no longer repeats them for factory pushes (TEAMS:2027-2029, cas-eac2), even though the fast-rows-pre-push.sh:5-7 header says CI owns the proof. Run the fast rows in the merge queue as a required check first. | none filed | DELETE (replace by required check / merge-queue fast rows) |
| H-16 | Factory process signal guard | PreToolUse, Bash, all factory roles | PT:575-587; PT:3313-3386 `worker_command_pattern_kill*` | Denies pkill, killall and kill on pgrep output. Prevents killing another worker's processes on the shared host (cas-a290). | SHELL-TEXT parse | Per-worker PID namespace (bwrap `--unshare-pid` on Linux) or a separate uid per worker on macOS | none filed | DELETE (replace by PID namespace or uid isolation) |
| H-17 | Neon production write | PreToolUse, MCP, worker | PT:663-678; cas-cli/src/hooks/handlers/handlers_events/neon_sql_guard.rs:143 `first_write_keyword` (SQL text), 211 `neon_production_write_denial` | Denies `run_sql`/`run_sql_transaction` writes resolving to the prod branch (GH #907). Coverage gap: no PreToolUse matcher names `mcp__neon__*` (TEAMS:1028-1034 + TEAMS:995-1011, config/hooks.rs:255-271, cli/hook/config_gen.rs:1135-1143), so only `mcp_execute` dispatch reaches it. | structured tool-arg + SQL-text classify | **Scoped token:** workers get only a branch connection string (db_branch_ops.rs:1-8 says the supervisor holds NEON_API_KEY and there is no Neon MCP upstream) | none filed | DELETE (replace by a branch-scoped Neon credential) |
| H-18 | Supervisor Agent(isolation=worktree) | PreToolUse, Agent, supervisor | PT:696-715 | Denies a supervisor spawning isolated-worktree subagents, which leak worktrees (cas-7c88). | structured tool-arg | Harness tool config cannot scope a single parameter | none filed | KEEP. One field, no false positives |
| H-19 | AskUserQuestion in factory | PreToolUse, AskUserQuestion, all factory roles | PT:729-745; matcher config/hooks.rs:270, TEAMS:1007 | Denies AskUserQuestion, which surfaces as a self-permission prompt and stalls the factory. | structured (tool name) | Harness tool config: `permissions.deny` or `--disallowedTools AskUserQuestion` in per-role factory settings (TEAMS:1028 builds those settings, but no deny list is emitted today) | none filed | DELETE (replace by harness tool deny) |
| H-20 | SendMessage, supervisor | PreToolUse, SendMessage, supervisor | PT:756-761 → PT:11727-11738 (GH #793) | Denies native SendMessage, which skips the durable CAS coordination queue. | structured (tool name) | Harness tool config deny for the supervisor role | none filed | MERGE into the harness tool deny |
| H-21 | SendMessage, worker | PreToolUse, SendMessage, worker | **PT:765-770** → **PT:11741-11751** ("SendMessage is disabled for factory workers", PT:11746); matcher config/hooks.rs:269, TEAMS:1006 | Denies every worker SendMessage whatever the target. Native subagents inherit the parent role (PT:574-576), so a worker cannot talk to its own in-process subagents. Defect it prevents: worker-to-peer traffic skipping the coordination origin stamps. | structured (tool name only; the target is ignored) | Target-aware structured check: allow when the target is the caller's own in-process subagent; deny factory peers and the supervisor. Or a harness setting that disables only team transport. | today about 9:55 AM ET (unfiled): a worker could not reach its own research subagents | MERGE (narrow to factory-peer targets) |
| H-22 | Worker commit guard / git scope (layer 1) | PreToolUse, Bash, worker | PT:787-849; PT:2341 `looks_like_git_write_op` → PT:2848 `git_write_targets` → PT:2794-2808 `git_history_write_operation` (**PT:2803: `checkout` with any non-dash argument counts as a write**); PT:3070 scope check; denials PT:3111 (outside worktree), PT:3149 (detached), PT:3217 (protected branch) | Denies a worker commit, merge, rebase, push, checkout or add -A outside its worktree or on main/staging (cas-bea2, cas-ba04). | SHELL-TEXT parse | Already doubled by the per-worktree git hooks H-39 and H-40; the PT:2338-2339 comment calls layer 2 "the hard floor". Add a GitHub ruleset so the worker token can push only `refs/heads/factory/*`. | GH #1752 (checkout in a scratch clone treated as commit), #1615 (false detached-HEAD) | DELETE (replace by H-39/H-40 plus a ruleset) |
| H-23 | Local-merge push deny | PreToolUse, Bash, worker | PT:804-819; PT:2645-2668 `looks_like_git_push_to_origin` (split_whitespace) | Denies `git push origin` in local_merge sessions (cas-6db4). | SHELL-TEXT (naive token scan) | Worker pre-push hook (H-40) reads the delivery mode; or no push credential or pushurl in local-merge sessions | none filed | MERGE into H-40 |
| H-24 | Independent QA merge (supervisor) | PreToolUse, Bash, supervisor | PT:858-870; cas-cli/src/qa_pass.rs:707 `factory_branches_merged_by`, 751 `supervisor_merge_refusal` | Denies raw `git merge factory/<w>` or `gh pr merge` while a QA-bound delivery lacks a passed round (cas-619f). | SHELL-TEXT parse | A Cassy-posted required GitHub status check ("independent-qa"); local merges through MCP `worktree_merge`, which already checks (qa_pass.rs:765) | none filed | MERGE (one merge gate as a required status check) |
| H-25 | GitHub raw merge QA (non-supervisor) | PreToolUse, Bash, worker/other | PT:876-887; qa_pass.rs github_merge_refusal; cas-cli/src/qa_pass/github_gate.rs:176 `github_merges_in` | Denies `gh pr merge` by any non-supervisor agent holding a gh token (GH #1023). | SHELL-TEXT parse | Scoped token: the worker gh token has no merge permission, plus the required status check | none filed | DELETE (replace by scoped token + ruleset) |
| H-26 | Peer-ask merge hold | PreToolUse, Bash, supervisor | PT:891-908; cas-cli/src/peer_ask_gate.rs:145 | Denies a supervisor PR merge while its own peer ask about that PR is unanswered (cas-bddf9). | SHELL-TEXT parse (through github_gate) | Required status check or merge only through MCP | GH #1634 (refuses one unlinked ask at a time, including answered ones) | MERGE into the H-24 merge gate |
| H-27 | Epic PR draft | PreToolUse, Bash, all roles | PT:912-928; cas-cli/src/ci_waste.rs:18-40 (split_whitespace) | Denies `gh pr create` to an epic without `--draft`. Prevents CI cost (cas-fee3). | SHELL-TEXT (naive) | CI workflow condition on the base ref (skip full CI for PRs into `epic/*`), or auto-draft through the GitHub API | none filed | DELETE (replace by a workflow `if:` on the base branch) |
| H-28 | Workspace contract, root-less fallback | PreToolUse, file tools + Bash, factory | PT:940-971 | Same as H-29 when no Cassy root resolves (bound to CAS_CLONE_PATH). | SHELL-TEXT + structured | Same as H-29 | (shares H-29) | MERGE into H-29's replacement |
| H-29 | Factory workspace contract / write roots | PreToolUse, Write/Edit/NotebookEdit/apply_patch/Bash, factory | PT:1022-1078; PT:5537 `factory_write_decision`; SE:18 `collect`; PT:4732 `bash_write_targets`; PT:4940 `bash_delete_targets`; PT:5048 `bash_link_creations`; PT:4238 `script_write_targets`; PT:7219 in-place edits; PT:7481 apply_patch; PT:5315 delete and runtime-file rule; PT:7316 operator policy; write_roots.rs:101 | Denies creating, editing or deleting outside the worktree, artifacts root, scratch root or harness scratchpad, plus deleting live sockets and locks and writing policy paths (GH #196, #528, #1169). | SHELL-TEXT parse (a bounded shell recognizer, SE:1-2) + structured | **Scoped worktree FS sandbox:** Codex `sandbox_mode=workspace-write` with `writable_roots`; Claude Code sandbox filesystem write allow list; bwrap read-only binds on Linux. Write roots become sandbox mounts. | GH #1218, #1284, #1290, #1396, #1535, #1583, #1590, #1603, #1711 (9 in 30 days) | DELETE (replace by the harness FS sandbox) |
| H-30 | Worktree merge jail | PreToolUse, all tools, non-factory with worktrees.enabled | PT:1176-1268 | Denies every tool while the agent's task has pending_worktree_merge. The code comment marks it "EXPERIMENTAL". | structured store state | MCP close path | none filed | DELETE if worktrees.enabled is unused (verify) |
| H-31 | Worktree redirect | PreToolUse, file tools, non-factory with worktrees.enabled | PT:1270-1360 | Denies file operations in the main repo when the epic has a worktree. | structured (path prefix) | FS sandbox | none filed | DELETE together with H-30 |
| H-32 | CODEMAP freshness gate | PreToolUse, MCP task/factory, supervisor | PT:1121-1172; PT:11599 `is_codemap_gated_tool_call`; codemap.rs:371-375 `evaluate_codemap_at`; docs_freshness.rs:38 | Denies supervisor task create or spawn_workers when CODEMAP.md is significantly stale. | structured (tool + action) + git measurement | MCP-side check in the spawn_workers / task-create handler | none filed | MERGE (move into the MCP handler; this is an MCP gate placed in the hook) |
| H-33 | Protection files/patterns (.env, keys) | PreToolUse, file tools + Bash, all | PT:1463-1520 (substring `path.contains(pattern)` at PT:1499, 1512); config `hooks.pre_tool_use.protection` | Denies paths matching protected file and credential patterns. | structured + SHELL-TEXT (Bash write targets) | FS sandbox deny-read and mask (same as H-09 and H-29) | none filed | MERGE into the sandbox and credential scoping |
| H-34 | Duplicate task-verifier | PreToolUse, Agent, factory | PT:1525-1546 | Denies a second verifier while the claimed one is live (cas-beaf). | structured + store | MCP dispatch state | none filed | KEEP. State-based, structured |
| H-35 | Verifier spawn handoff gate | PreToolUse, Task/Agent subagent_type=task-verifier | PT:1548-1736; close_ops/gate_text.rs `verifier_spawn_denial` | Denies a verifier spawn without a unique task or an owned, unexpired dispatch, and issues the sealed handoff (cli/hook/config_gen.rs:430 says this is the only route that issues it). | structured + store | none, keep | none filed | KEEP. Structural; issues the credential |
| H-36 | Exit blockers | Stop, non-factory sessions | cas-cli/src/hooks/handlers/handlers_middle/session_stop/stop_flow.rs:56-97 (block at 89; `stop_hook_active` brake at 41-51) | Blocks stop while open tasks or children lack a handoff. | structured store state | none, keep | none filed | KEEP |
| H-37 | Loop iteration continue | Stop | .../session_stop/mod.rs:129 `block_stop_with_context` | Blocks stop to run the next loop iteration (feature, not a safety guard). | structured | none, keep | none filed | KEEP |
| H-38 | Main-checkout pre-push | git pre-push (core.hooksPath=.githooks) | .githooks/pre-push:1-10 → scripts/fast-rows-pre-push.sh:1-119 (override `CAS_SKIP_FAST_ROWS`, lines 10-11); release-gate.sh `--fast-rows` (version-literals, test-shape and others, release-gate.sh:21-23); installer scripts/install-git-hooks.sh | For `refs/heads/factory/*`, requires a same-tip fast-rows PASS. | git hook (content checks) | Required CI check (see the H-15 caveat) | Fast-row content false positives: GH #1557 (version-literals), #1565, #1630, #1631, #1679, #1719 (test-shape / changed-string) | KEEP as local feedback. Content-check false positives are out of scope here |
| H-39 | Worker pre-commit guard (layer 2) | git pre-commit, per-worktree hooksPath | TEAMS:1926-1962 script; TEAMS:2277 install; TEAMS:2301-2324; TEAMS:2364-2374 `config --worktree core.hooksPath` | Refuses a commit unless HEAD is `factory/$CAS_AGENT_NAME` or its `-<task>` branch. | git hook (capability-like: git-enforced; `--no-verify` skips it) | Pair with a ruleset | none filed | KEEP. Becomes the primary commit fence once H-22 goes |
| H-40 | Worker pre-push guard | git pre-push, per-worktree hooksPath | TEAMS:1969-2056 (ownership 1992-1997, ref check 2004-2012, non-FF 2016-2023, fast rows 2027-2034, `factory.worker_local_gate` 2037-2053); install TEAMS:2326-2362 | Refuses pushing another branch, a cross-task ref, or a non-fast-forward over a parked delivery; runs fast rows and the local gate. | git hook | GitHub ruleset (restrict worker token to `factory/*`, block force-push) as the server-side floor | GH #1386 (auto-push ran the repo pre-push against the supervisor main tree) | KEEP. Absorb H-23 |
| H-41 | Operator-only CLI context check | CLI `cas config set write_roots / grant-write / revoke-write` | cas-cli/src/cli/config/operator_ops.rs:58, 75-92, 106, 281, 325; cas-cli/src/config/operator_policy.rs:963-976 | Refuses from an agent environment, agent ancestor, factory cgroup or no TTY, and requires typed confirmation. | capability-like (process context + TTY) | (this is the replacement for H-11 and H-12) | none filed | KEEP |
| H-42 | `cas mcp` CLI supervisor-only guard | CLI | cas-cli/src/cli/mcp_cmd.rs:227, 421-440 | Refuses agent removal or aliasing of supervisor-only servers. | capability-like (InvocationContext) | (replacement for H-13) | none filed | KEEP |
| H-43 | MCP proxy supervisor-only routes and env | MCP server (`cas serve`) | cas-cli/src/mcp/server/runtime.rs:410-416 (withholds `supervisor_only_env` from the worker process), 623-633 (`supervisor:` routes, GH #988) | Workers cannot call supervisor routes or receive their credentials. | MCP role check + credential scoping | (target model for H-10 and H-17) | none filed | KEEP |
| H-44 | MCP proxy add/remove of supervisor-only servers | MCP `system` tool | cas-cli/src/mcp/tools/service/agent_search_system/system.rs:725, 810; operator_policy.rs:900, 939 | Refuses removing, replacing or aliasing a supervisor-only upstream. | MCP role check | none, keep | none filed | KEEP |
| H-45 | MCP lifecycle role gates (bundle) | MCP tools | close_ops.rs:4941 (request_changes), 13031 (reopen); task/update.rs:1632 (priority); qa_pass_ops.rs:225, 307; factory_ops.rs:2333 (hold); message.rs:1388 (broadcast); shadow_review/mod.rs:221; core/artifact.rs:30-44 (revoke); db_branch_ops.rs:117, 208, 251 (all under cas-cli/src/mcp/tools/) | Supervisor-only actions are refused for workers, keyed on the registered agent row. | MCP role check | none, keep | (close-gate false positives exist but are not pre-action guards) | KEEP. This is the pattern to move the other gates toward |
| H-46 | Worker store isolation (bwrap) | OS sandbox at worker spawn | crates/cas-pty/src/store_isolation.rs:1-18, 29-34, 38; crates/cas-pty/src/pty.rs:2569, 4303 | Masks the Neon/Vercel store dirs with an empty read-only tmpfs for worker launches on Linux; macOS warns and relies on H-09. | capability | (replacement for H-09; extend to macOS) | none filed | KEEP and extend to macOS |

Table: Shell and text recognizers feeding the guards

| # | Recognizer | Location |
|---|---|---|
| 1 | Shared lexer `factory_shell_lex`; `shell_statement_words` / `_arguments` | PT:3469, 3764, 3771 |
| 2 | `shell_invocations` / `collect_shell_invocations` / substitutions | PT:5954-6160 |
| 3 | `shell_effects::collect` (a bounded shell recognizer) | SE:18 |
| 4 | `bash_write_targets` | PT:4732 |
| 5 | `bash_delete_targets` | PT:4940 |
| 6 | `bash_link_creations` | PT:5048 |
| 7 | `bash_in_place_edit_targets` | PT:7219 |
| 8 | Python/Node script scanner `script_tokens` / `script_write_targets` | PT:4018, 4238 |
| 9 | Heredoc parser | PT:4470, 4598 |
| 10 | `git_write_targets` / `git_history_write_operation` | PT:2848, 2794 |
| 11 | Git env/hooksPath bypass scan | PT:2359 |
| 12 | `looks_like_git_push_to_origin` (split_whitespace) | PT:2645 |
| 13 | `worker_check_command` / literal form / log regex | PT:1764, 1815, 1882 |
| 14 | `rust_build_invocation` and the Makefile parser | PT:1993, 2144 |
| 15 | `formatter_invocation_can_spill` | PT:2245 |
| 16 | `worker_command_pattern_kill_at_depth` | PT:3321 |
| 17 | Operator-policy textual and clap parse, plus read-only allowlists | PT:6255, 6405, 6476-6989 |
| 18 | `role_env_tampering_at_depth` | PT:7117 |
| 19 | `cas_invocations` | PT:5942 |
| 20 | Credential-store shell/statement/script matcher (keeps its own cd/pushd stack) | OC:95, 163, 780 |
| 21 | `browser_tier_guard::unfiltered` | browser_tier_guard.rs:120 |
| 22 | `epic_pr_draft_refusal` (split_whitespace) | ci_waste.rs:18 |
| 23 | Merge parsers `factory_branches_merged_by` and `github_merges_in` | qa_pass.rs:707; github_gate.rs:176 |
| 24 | SQL write classifier | neon_sql_guard.rs:143 |
| 25 | mcp_execute `server.tool(...)` code parsers | WDT:186; slack_transport.rs:55 |

</details>

<details open data-collapse>
<summary>B. Guard false positives, October 11 to November 10</summary>

Table: Action-guard false positives (18 filed, 3 observed today)

| # | Filed (ET) | State | Title (short) | Guard |
|---|---|---|---|---|
| 1218 | Oct 15, 5:19 PM | closed | Workspace contract refuses scratch path with `$$`/`$VAR` | H-29 |
| 1284 | Oct 18, 2:44 AM | closed | Workspace contract blocks the support loop's state.json under home | H-29 |
| 1290 | Oct 19, 9:48 AM | closed | Workspace contract refuses write-free `gh --body "$(cat <<EOF)"` | H-29 |
| 1366 | Oct 26, 8:35 AM | closed | BROWSER TEST TIER refuses every targeted Playwright run | H-08 |
| 1386 | Oct 27, 10:54 AM | closed | worktree_merge auto-push runs repo pre-push against the supervisor main tree | H-40/H-38 |
| 1396 | Oct 27, 1:04 PM | closed | Write guard blocks the harness's own auto-memory dir | H-29 |
| 1535 | Nov 5, 3:46 PM | closed | Shell effect parser rejects a read-only quoted regex | H-29 |
| 1557 | Nov 6, 6:43 AM | closed | Worker pre-push version-literals rejects a Vite fixture | H-38/H-40 |
| 1583 | Nov 6, 10:28 AM | closed | Workspace hook treats quoted `=>` in a grep regex as a redirect | H-29 |
| 1590 | Nov 6, 11:06 AM | closed | Workspace hook rejects an exit-status assertion as arithmetic | H-29 |
| 1603 | Nov 6, 12:24 PM | open | Artifact write hook rejects realpath of a symlinked artifacts root | H-29 |
| 1615 | Nov 6, 2:35 PM | open | Worker commit guard reports detached HEAD on a named branch | H-22 |
| 1634 | Nov 6, 6:51 PM | closed | Merge guard refuses one unlinked peer ask at a time | H-26 |
| 1694 | Nov 9, 3:20 AM | closed | Capped nextest rejects isolated HOME env prefixes | H-02 |
| 1700 | Nov 9, 5:42 AM | open | Credential hook blocks skill-reference `find` under home | H-09 |
| 1711 | Nov 9, 11:27 AM | closed | Shell effects rejects `test "$?" -eq 1` | H-29 |
| 1752 | Nov 10, 9:07 AM | open | Commit guard rejects a checkout with no commit | H-22 |
| 1754 | Nov 10, 9:16 AM | open | Read-only SSH DB query rejected as credential access | H-09 |
| (unfiled) | Nov 10, ~9:43 AM | n/a | Home-dir `find -maxdepth 6` denied | H-09 |
| (unfiled) | Nov 10, ~9:55 AM | n/a | Worker SendMessage to own subagents denied | H-21 |
| (unfiled) | Nov 10, 10:06 AM | n/a | Heredoc writing this inventory denied (doc text names store paths) | H-09 |

Table: False positives by guard

| Guard | Count | Notes |
|---|---|---|
| H-29 workspace contract | 9 | |
| H-09 credential store | 4 | 2 filed + 2 today; origin GH #1293, Oct 19 |
| H-22 commit guard | 2 | |
| H-38/H-40 pre-push | 2 action + 5 content | |
| H-08 browser tier | 1 | |
| H-26 peer-ask hold | 1 | |
| H-02 worker check | 1 | |
| H-21 SendMessage | 1 | today |

</details>

<details open data-collapse>
<summary>C and D. Abbreviations and target projections for watchdogs and notices</summary>

Abbreviations (full path = prefix + rest):

- `rt/` = `cas-cli/src/ui/factory/daemon/runtime/`; `qe` = `rt/queue_and_events.rs`; `lcy` = `rt/lifecycle.rs`; `pw` = `rt/progress_watchdog.rs`
- `dir/` = `cas-cli/src/ui/factory/director/`; `app/` = `cas-cli/src/ui/factory/app/`
- `lc/` = `cas-cli/src/mcp/tools/core/task/lifecycle/`; `sp` = `lc/supervisor_push.rs`; `co` = `lc/close_ops.rs`
- `hs` = `cas-cli/src/hooks/handlers/handlers_session.rs`; `bs` = `crates/cas-core/src/hooks/context/build_start.rs`; `fi` = `cas-cli/src/hooks/handlers/handlers_middle/factory_inbox.rs`; `sh` = `cas-cli/src/hooks/session_hygiene.rs` (path as cited by the scan); `orph` = `cas-cli/src/mcp/tools/service/orphan_recovery.rs`

Target projections named in the verdicts:

- **PROJ-owed**: the next owed action per open item (merge, close, verdict, assign, operator lift or decision), keyed (task, action, as-of SHA). The surviving row is W-03's `SupervisorActionableState`, rebuilt from stored facts; its MCP-silence timer is dropped.
- **PROJ-delivery**: delivery facts (parked, merged, contained, released), keyed (task, SHA, state). Survivors: N-24 (park) and W-45 (merged).
- **PROJ-liveness**: real loss of an agent, harness or daemon, keyed (agent or process, incident). Survivors: W-12, W-19, N-08, W-58.
- **PROJ-message**: delivery state of one message, keyed message id. Survivors: W-27 (no turn started), W-31 (unread), W-34 (supervisor channel lost, escalated to the operator).

</details>

<details open data-collapse>
<summary>C. All 69 watchdogs</summary>

Table: Watchdogs W-01 to W-69

| ID | Name | Trigger and cadence | Source file:line @01df2dc77 | Condition detected / what it says | Recipient | Source it reads | Dedup/rate-limit | Duplicates | Known FPs / floods | Verdict |
|---|---|---|---|---|---|---|---|---|---|---|
| W-01 | Progress watchdog supervisor wake (`progress_stalled`) | 60s poison sweep (qe:40, qe:4762-4766); fires after 45 min with no change (pw:15 `QUIET_SECS`) | pw:178-205 (observe pw:32-128) | "Nothing has moved in this factory for N min… If nothing moves in another 45 min, the operator is paged" | supervisor | RE-POLL: task store, latest message id, `git for-each-ref` lane/train tips (pw:116-125); exemptions re-derive freeze, contained delivery and peer split | in-memory EPISODES (pw:19) + durable key `progress-stall:{start}` (pw:198) | W-03, W-06, W-09, W-04 | cas-1f65 (frozen train), cas-c1fb (peer train, 13 contained deliveries, 2 fires), cas-098dd (gate-refused epic close; supervisor forced to mark 3 tasks blocked), cas-e239 (operator-blocked / held) | MERGE(W-03): absence-of-activity heuristic; each of 4 FP fixes adds a stored fact it should have read (freeze, containment, split, refused close) |
| W-02 | Progress watchdog operator page | same sweep; 2×QUIET_SECS = 90 min | pw:205-215 | "Nothing has moved in factory session S for N min… Supervisor X was woken at 45 min" | operator (Commander blocker) | RE-POLL (as W-01) | key `progress-stall-page:{session}:{start}` (pw:213); episode state is process-local (pw:19) | W-04 (2h idle page, different key) | cas-1f65 (threatened overnight page), cas-c1fb, cas-098dd (red test: page at 90 min) | MERGE(W-03): PROJ-owed pages the operator only when the owed action's owner is the operator or a dead agent |
| W-03 | Director SupervisorStalled (actionable-idle wake) | 2s director refresh (lcy:5); fires after supervisor MCP silence ≥ `stall_after_secs` 600 (crates/cas-factory/src/config.rs:167); refires every 600s (dir/events.rs:18) | dir/events.rs:318; built app/mod.rs:2074-2135; relayed lcy:287-372 | "Supervisor actionable-idle for Nm. Drive to the exit: …" in 6 states: MergeBranches, MergeCloseBlocked, MergeQueue, ReleaseVerdictDue, AssignReadyWork, AssembleGatePipeline (dir/events.rs:70-123) | supervisor | DERIVED snapshot (tasks, agents, reminders) + RE-POLL git/CI evidence (W-49); revalidated at send (app/mod.rs:2138-2172) | tracker reset on activity (dir/events.rs:275-281); once per merged/verdict state (dir/events.rs:188-197); assign cooldown 30 min (dir/events.rs:21); 30s debounce key `supervisor_stalled:{now}` never matches (dir/events.rs:1251) | W-01, W-09, W-47, W-48, N-05 | cas-8391 (every 10 min under operator-owned blocker), cas-4175 (GH #835 merge re-demand on merged branch), cas-7809 (peer-owned deliveries), cas-7310 / cas-728b (nag cleanup) | KEEP: becomes PROJ-owed (states are already fact-derived); swap the silence timer for a per-(task, action, SHA) key |
| W-04 | Supervisor-idle operator page | on a W-03 wake when last supervisor call ≥ 7200s (dir/events.rs:26, :336-338) | lcy:63-95 → `record_operator_page` lcy:98-153 | "Supervisor X has taken no action for over 2 hours… Type in its pane to wake it" | operator | DERIVED (W-03 snapshot) | key `supervisor-idle-page:{session}:{episode}` (lcy:89); skipped under project hold (lcy:298) | W-02 (one quiet night can page twice) | none recorded | MERGE(W-03): a second operator page for the same quiet episode |
| W-05 | WorkerStalled nudge (to worker) | 2s refresh; activity older than `stall_threshold_secs` 300 × effort (config.rs:163; dir/events.rs:1025-1039); heartbeat <60s; transcript confirm (dir/events.rs:1541-1612) | dir/events.rs:2709-2729; text dir/prompts.rs:2043-2050; durable lane lcy:26-44 → rt/delivery.rs:535-555 | "You have gone quiet on task T for about Nm (heartbeat is fine…)" | worker | RE-POLL transcript mtime/size, in-flight tool, /proc children, worktree HEAD | once per streak; `admit_stall_alarm` (dir/events.rs:2206-2224); key `worker-attention:{session}:{episode}` (rt/delivery.rs:544-546) | W-06, W-01, W-10 | cas-7156 (67 nudges to one worker in long commands), cas-058e (blind to background jobs), cas-9829 (origin) | KEEP: the only signal for a hung harness turn behind a live heartbeat (PROJ-liveness); one episode key with W-06 |
| W-06 | WorkerStalled escalate (`worker_stalled` relay) | the next 2s tick after W-05 while still confirmed; no minimum delay (dir/events.rs:2730-2738) | dir/events.rs:2743; relay lcy:376-427 | "Worker W is stalled on T after Nm without activity." (the triage text at dir/prompts.rs:2086-2098 is never sent) | supervisor | RE-POLL (as W-05) | `attention_episode_key` (lcy:405) + StaleAlert ledger (lcy:407-414) | W-05, W-01 | cas-0028 (permanent keys hid later episodes), cas-7156 | MERGE(W-05): same episode; escalate on one key after a real delay |
| W-07 | WorkerStalled assigned-but-unstarted | 2s refresh; assigned Open task, quiet ≥ threshold, transcript confirms | dir/events.rs:2513-2576 (emit :2565) | sends the W-06 text (the richer dir/prompts.rs:2018-2024 text is unused) | supervisor | RE-POLL (as W-05) | `stall_escalated` + `admit_stall_alarm` | W-09, W-03 AssignReadyWork | cas-8a55 (auth-dead workers looked "assigned but unstarted" for 34 min) | MERGE(W-05) |
| W-08 | WorkerStalled released-never-woken | 2s refresh; no activity since a supervisor hold release ≥ threshold | dir/events.rs:1916-1944 (emit :2586) | W-06 text | supervisor | DERIVED activity rows only; no transcript confirmation | one-shot (`released_workers` entry removed); bypasses `admit_stall_alarm` | W-06 | none recorded | MERGE(W-05): an unconfirmed absence heuristic |
| W-09 | WorkerIdle (no task, `worker_idle` relay) | 2 ticks (~4s; dir/events.rs:1057) after a 10s grace (:1063) | dir/events.rs:2885; relay lcy:397 | "Worker W is idle with no active task and needs supervisor attention." | supervisor | DERIVED rows + RE-POLL harness liveness (dir/events.rs:2856-2863) | per streak; `IDLE_RATE_LIMIT` 300s (dir/events.rs:1002, :3112); episode key | W-03 AssignReadyWork, W-11, N-04 | cas-0028 | MERGE(W-03): idle worker + ready work is the "assign" owed action |
| W-10 | QA walk "qa-flat" | each refresh; QA ledger `progress_at` older than 300s | cas-cli/src/qa_walk_progress.rs:121-129; wrapped app/director_refresh.rs:805-808 | "QA output flat: W / T / ledger: no recorded result for Ns" | supervisor | RE-POLL ledger files + clock | key `qa-flat:{task}:{ledger}:{stamp}` + 30s debounce | W-05 | none recorded | MERGE(W-05): a stall heuristic for QA workers |
| W-11 | QA walk "qa-split" | each refresh; one QA task left + idle worker | cas-cli/src/qa_walk_progress.rs:177-180 | "QA critical path: only T remains; W is idle. Consider splitting" | supervisor | RE-POLL liveness + prompt queue | key `qa-split:{task}:{idle}` | W-09, W-03 | none recorded | MERGE(W-03): an owed action (split/assign) |
| W-12 | Spawn registration timeout → "worker harness failed during boot" | 2s tick (lcy:4667); 60s with no registration (qe:49) | qe:6544 `reconcile_spawn_verifications`; timeout qe:6678-6687; failure path qe:6718-6750 (reason text qe:6739, `DeathRelay::Alarm`) | "did not register with Cassy within 60 seconds" + pane tail; kills PTY, releases preassign, stale-marks agent; Critical spawn-outcome blocker (qe:1836-1846) **and** Critical `worker_died` | supervisor + desktop | RE-POLL registry, PTY buffer, clock | one-shot (verification entry removed) | W-20 (90s reap of the same failure), N-08 | **cas-c74f8**: a requested shutdown during boot was relayed as harness failure (notification 101899) because the watch survived the shutdown; covered by cas-7076 (4058c34ac) | KEEP: real liveness loss (PROJ-liveness); must consult the stored shutdown request; send one Critical notice, not two |
| W-13 | Boot model error | 2s tick before registration | qe:675-693, checked qe:6574 | "Harness reported boot model error (…)" | supervisor | RE-POLL pane | one-shot | W-12 (same path) | none recorded | KEEP: a cause code of W-12 |
| W-14 | Codex hook-review screen | 2s tick before registration | qe:699-723, checked qe:6578-6583 | `CODEX_HOOK_REVIEW_FAILURE` + pane tail | supervisor | RE-POLL pane | one-shot | W-12 | none recorded | KEEP: a cause code of W-12 |
| W-15 | Hook canary failed | SessionStart canary missing after 60s (cas-cli/src/factory_hook_canary.rs:17) | qe:6591-6612 | canary failure detail + boot output | supervisor | RE-POLL canary file + pane | one-shot | W-12 | none recorded | KEEP: a cause code of W-12 |
| W-16 | Post-registration exit / disappearance | 60s window (cas-cli/src/mcp/tools/service/factory_ops.rs:477) | qe:6634-6653, qe:6662-6677 | "Worker harness process exited / disappeared during post-registration boot verification" | supervisor | RE-POLL PID + registry | one-shot | N-08 | none recorded | KEEP: a cause code of W-12 |
| W-17 | Spawn provisioning timeout | 300s (qe:60) | qe:11646-11712 | "Worktree provisioning for worker W did not finish within 300 seconds" (Critical spawn outcome) | supervisor | clock + task handle | one-shot | none | none recorded | KEEP: the only signal for a hung `git worktree add` |
| W-18 | Spawn queue stall scan | every 30s (qe:62); rows older than 60s (qe:64) | qe:11369-11394 → rt/queue_control_jobs.rs:289-297 | "Spawn request N … queued … without being dequeued" | **nobody**: log line + audit row only | DERIVED spawn queue | in-memory `reported_stalled_spawn_requests` (qe:11393), cleared qe:11329 | W-58 (job wedge) | none recorded | DELETE: no recipient; a stuck spawn lane already shows up as a W-58 job wedge |
| W-19 | Heartbeat-stale reap (orphan recovery) | maintenance loops; heartbeat >600s | orph:484-542 (row :823-855, prompt :859-937); loops cas-cli/src/daemon/maintenance.rs:310-347 **and** cas-cli/src/mcp/daemon.rs:1868-1926 | parks tasks; WorkerDied event + Critical `worker_died` relay | live supervisor/director, else parent (orph:563-585) | DERIVED heartbeat + RE-POLL process (orph:224) | incident key `worker_died:{id}:{last_heartbeat_ms}` (orph:802-808) | W-12, W-20, W-21, N-08 (same death, different detectors) | none recorded | KEEP: real liveness loss (PROJ-liveness); run one loop, not two |
| W-20 | Failed-startup reap | `list_failed_startup(90)` | cas-cli/src/daemon/maintenance.rs:267-305 (reason :299) | "daemon maintenance: failed startup" → W-19 relay | supervisor | DERIVED registry rows | incident key | W-12 (60s) | none recorded | MERGE(W-12): same boot failure, seen 30s later |
| W-21 | Expired lease, dead holder | maintenance loops; 600s | orph:244-282; called maintenance.rs:363, mcp/daemon.rs:1943 | "lease expired while holder gone" → W-19 relay | supervisor | leases + heartbeat + RE-POLL process | incident key | W-19 | none recorded | MERGE(W-19) |
| W-22 | Correlated worker deaths fold (`workers_died_together`) | exits within 5s (qe:288) | qe:6355-6390; `relay_death_group_io` qe:384-425; relay lcy:501-586 (redrive lcy:645-689; fold copy lcy:588-643) | "N worker harnesses exited within X ms… treat as one incident" | supervisor | DERIVED in-memory exits | `death_group_key` (qe:370-375); occurrence `{worker}:{ms}` (lcy:557) | replaces per-worker N-08 | none recorded | KEEP: folds N deaths into one incident (PROJ-liveness) |
| W-23 | Death sweep re-drive | every 5s (qe:6422); rows ≤30 min (qe:294) | qe:6420-6455; `read_death_sweep` qe:545-622; orph:715 | re-injects `worker_died` prompts that never reached the queue; gives up with a log line | supervisor | DERIVED supervisor_queue | notification id; `death_sweep_given_up` (qe:6452) | W-37 (generic outbox re-driver) | none recorded | MERGE(W-37): one outbox re-driver |
| W-24 | Usage-limit scan (`worker_unavailable`) | every 60s (qe:4108) | qe:4107-4213 → lcy:458-463 | usage-limit detail, or "Worker W hit its usage limit; reset time unavailable" (qe:4195) | supervisor | RE-POLL transcript/rollout | in-memory map + durable episode key (qe:4184-4200) | none | none recorded | KEEP: the harness heartbeats while dead; no other signal |
| W-25 | Harness auth-failure scan | every 15s (qe:4607) | qe:4606-4741 (notice qe:4720-4729) | releases the worker's tasks, marks the spawn failed, sends "harness_auth" | supervisor | RE-POLL transcript (cas-cli/src/factory_auth_health.rs) | **in-memory only** (qe:4676) | none | cas-8a55 (origin incident) | KEEP: real liveness loss; dedup must be durable |
| W-26 | Permission approval pending | scan every 5s (qe:4464); request older than 120s (cas-cli/src/factory_permission_relay.rs:38) | qe:4461-4590 → lcy:691-719 | "Worker W has waited Ns for a TOOL permission… approve/deny" | supervisor | RE-POLL Teams mailbox file | request id key (lcy:717) + in-memory `reported_permission_requests` | none | none recorded | KEEP: the only signal for a parked permission |
| W-27 | Normal-delivery watchdog (pane nudge, then flag) | 30s observe window (qe:3332); renudge 60s (qe:2348) | qe:17-18 (nudge text), qe:3509-3545, qe:5350-5395 | types "Cassy delivery watchdog: a normal supervisor message may be waiting…", then flags the supervisor "no turn started after delivery" | recipient pane, then supervisor | RE-POLL PTY / harness turn evidence | per probe | W-28, W-29, W-31, W-33 | cas-40ed (GH #1687 repeated wakes for an empty inbox), cas-6e76 | KEEP: PROJ-message survivor for "delivered, no turn started" |
| W-28 | Worker delivery stalled (`worker_delivery_stalled`) | after W-27's bounded retry | rt/delivery_probe_jobs.rs:128, :164 → lcy:436-456 | "Worker W started no harness turn after normal message N and its bounded retry" | supervisor | RE-POLL pane/transcript | `delivery-window:{5-min bucket}` (lcy:453) | W-27, W-33 | none recorded | MERGE(W-27) |
| W-29 | Claude wake budget exhausted | after the bounded redelivery budget | qe:5807-5830 | `<system-notice>Claude worker wake failed: notification_id…` | supervisor | DERIVED queue row + transcript silence | keyed per message (cas-d1659 comment, qe:5826-5829) | W-27, W-35 | none recorded | MERGE(W-27) |
| W-30 | Teams pointer body fallback | poison sweep; pointer unexpanded after 180s (qe:43) | qe:4840; rt/poison_sweep_jobs.rs:165-171 | writes the full body as its own teammate message (auto-act) | worker Teams inbox | DERIVED prompt_queue | `claim_teams_body_fallback` (rt/poison_sweep_jobs.rs:165) | N-87 | none recorded | KEEP: delivery mechanism, not an alarm |
| W-31 | Sender delivery-stalled bounce | poison sweep; config `delivery_stalled_{priority,normal}_secs` (rt/poison_sweep_jobs.rs:244-246) | qe:4951-5010; rt/poison_sweep_jobs.rs:233, :268 | "delivery stalled — recipient unread" | sender | DERIVED prompt_queue + transcripts | once per recipient episode (crates/cas-store/src/prompt_queue_store.rs:7218); `sender-bounce:{id}` (qe:5006) | W-32, W-33, W-69 | none recorded | KEEP: PROJ-message survivor for "unread", one key per message |
| W-32 | Supervisor unread backlog (`supervisor_unread`) | poison sweep | qe:4980-4997 → lcy:1505-1530 | "N messages addressed to you have gone unread… oldest message N from X" | supervisor | DERIVED prompt_queue | `supervisor-unread:{oldest_id}` (lcy:1520) | W-31, W-34 | cas-37b8 (counted FYI-only rows), cas-f265 (self-reminder raised unread) | MERGE(W-31) |
| W-33 | Idle-unacked relay | after 5 min (qe:3779) | qe:5057-5090 → rt/delivery_probe_jobs.rs:183, :237 | "Unread message on an idle worker" | supervisor | DERIVED queue + turn completion | `idle-unacked:{id}` (rt/delivery_probe_jobs.rs:278) | W-31, W-28 | none recorded | MERGE(W-31): one message can raise 4 notices (W-31, W-69, W-33, W-28) |
| W-34 | Relay-watchdog: lifecycle relay never reached the supervisor | poison sweep; older than 540s (qe:3807) | qe:3876-3930, record qe:4000-4035; daemon wrapper + desktop qe:5101 | "T is awaiting merge… the supervisor never received" (Blocker) | operator | DERIVED prompt_queue delivery state + task still current (qe:3931) | `relay-escalation:{prompt_id}` (crates/cas-store/src/prompt_queue_store.rs:114) | W-35, W-39, N-24 | cas-20ac (lifecycle relay flood starving supervisor) | KEEP: PROJ-message survivor for "supervisor channel dead" → operator |
| W-35 | Undelivered worker-death notice | 20 renudges (qe:2391) at 60s (qe:2348) | qe:10313-10329 → rt/prompt_queue_jobs.rs:254-270 | "UNDELIVERED: the factory tried to tell the supervisor that worker W died…" | supervisor | DERIVED | `undelivered-lifecycle-relay:{row}` | W-34 | none recorded | MERGE(W-34) |
| W-36 | Stale prompt quarantine (poison-store sweep) | poison sweep; 24h TTL (crates/cas-store/src/prompt_queue_store.rs:92) | qe:4772-4815 | abandons aged rows for departed targets; quarantines stale rows (log only) | none | DERIVED prompt_queue | none (idempotent state change) | none | cas-d047 (GH #69, months-late delivery) | KEEP: housekeeping, no notice |
| W-37 | Lifecycle outbox drain | every prompt tick (lcy:4242, :4410) | lcy:1226-1257 → sp:1882 | replays undelivered `task_lifecycle` rows | supervisor | DERIVED supervisor_queue | `lifecycle-outbox:{id}` (sp:215); `submit_unique` | W-23 | cas-20ac | KEEP: the single re-driver for stored relays |
| W-38 | Verification deadline extension | 60s sweep; 2 min lead, +10 min, 60 min cap | lcy:156-220 | silently extends a dispatch deadline while the verifier heartbeat is under 3 min old or its lane is active | none (auto-act) | RE-POLL verifier heartbeat + lane files | cap via `extend_verification_dispatch_deadline` | W-39 | none recorded | MERGE(W-39): part of one expiry watchdog |
| W-39 | Expired verification dispatch relay (`verification_dispatch_expired`) | 60s sweep (qe:4755-4759); 20 per sweep (lcy:227); 24h window (sp:1253) | lcy:222-285 | "Verification dispatch X for T passed its deadline… Re-dispatch the verifier" | supervisor | DERIVED stored `deadline_at` | relay key per dispatch id (lcy:247-255) | W-41 (blocked-deadline alert for the same dispatch) | **cas-cdab** (open): one expired dispatch wakes the supervisor twice (rows #33 + #34) | KEEP: survivor for dispatch expiry; absorb W-41's verification-wait branch |
| W-40 | Orphaned dispatch adoption | at supervisor registration | cas-cli/src/mcp/daemon.rs:492, :527 → crates/cas-store/src/verification_store.rs:1403 | silently re-owns a dead same-session supervisor's unresolved dispatch, +10 min deadline | none (auto-act) | RE-POLL PID liveness | CAS under write lock | N-13 | cas-4271 (crashed verifier retry) | KEEP: a fact-based ownership transfer |
| W-41 | Blocked-task alert reconcile | every 2s refresh with blocked tasks (lcy:5562) | sp:1290 (`reconcile_blocked_task_alert`); escalation after 5 min (sp:1333-1373) | first receipt, then one escalation "Blocker deadline reached or verification ended"; defers to W-39 under 24h (sp:1259) | supervisor | DERIVED task + dispatch | `transition_key` / outbox | W-39, N-20 | **cas-cdab** (task_blocked row #33 duplicates W-39 row #34) | KEEP for real blockers; MERGE its verification-deadline branch into W-39 |
| W-42 | CI red-run watcher | 60s (rt/ci_watch.rs:19); 30s while a wake is pending (:31, :36); scheduled lcy:4453 | rt/ci_watch.rs:1530, `emit_failure` :1689 | "CI FAILED: branch (sha)" / "BLOCKER: merged lane red in X" | supervisor | RE-POLL GitHub runs + `git branch --contains` (:1604) | `ci-red-run:{branch}:{sha}` (:74) | W-43, W-49 | none recorded | KEEP: GitHub is the source of truth for CI; record the result as a fact once |
| W-43 | PR-lane failure (`pr_lane_failed`) | same poll | rt/ci_watch.rs:1466 → lcy:1095-1120 | "Delivery PR #n failed Scoped Validation" | supervisor (worker-attention) | RE-POLL GitHub jobs | `pr-lane-failed:{pr}:{sha}` (rt/ci_watch.rs:91) | W-42 | none recorded | KEEP |
| W-44 | Merge-queue ejection + repair cycle | same poll; probe TTL 60s (rt/ci_watch.rs:46) | rt/ci_watch.rs:1349 → lcy:753-950; repair decide/apply lcy:955-1090; failed-job cache rt/merge_queue_ejection.rs:82 | "PR #n left the merge queue…"; 1st ejection auto `request_changes` (lcy:1049), 2nd pages the operator (N-28) | supervisor + worker inbox | RE-POLL GitHub GraphQL + `merge_group` runs vs in-memory prior membership | `merge-queue-ejection:{session}:{task}:{pr}:{occurrence}` (lcy:806), worker copy `merge-queue-ejection-worker:` (lcy:914) | W-62 (its cancellations surface here as "dequeued") | none recorded | KEEP: an external event; link W-62 cancellations to it |
| W-45 | Delivery PR merged observation + wake | same poll | rt/ci_watch.rs:1195 → lcy:1376-1426, lcy:1124-1225 | "PR #n merged: close T", or a recovery relay when no verdict | worker (supervisor on recovery) | RE-POLL GitHub for every awaiting-merge delivery, even after `delivery_pr_merge_commit` is stored (lcy:1403-1405) | `delivery-pr-merged:{task}:{pr}:{merge}` (lcy:1186, :1199) | W-46, W-47, W-51 (4 reporters of "merged, not closed") | none recorded | KEEP: PROJ-delivery survivor (emit once when the merge fact is recorded); stop re-polling stored merges |
| W-46 | Delivery PR merge timeout (`delivery_pr_merge_timeout`) | after every CI poll (lcy:1288); 5 min grace (rt/ci_watch.rs:48) | lcy:1428-1500 | "Delivery PR #n merged at X, but task remains S after Nm" | supervisor | DERIVED stored `delivery_pr_merged_at` (lists all tasks each poll) | stable key `delivery-pr-merged:…` (lcy:1458) | W-45, W-47 | none recorded | MERGE(W-45) |
| W-47 | Merged-close-blocked relay (`merged_close_blocked`) | part of W-03 (2s refresh) | lcy:309-326; key dir/events.rs:179; git evidence app/delivery_evidence.rs:383 | "Supervisor actionable-idle… delivery already merged; awaiting close" | supervisor | RE-POLL git ancestry every refresh | stable key per delivery (lcy:321) | W-45, W-46 | cas-4175, cas-ffd5 | MERGE(W-45) |
| W-48 | WorkerIdle MERGE REQUIRED merge alert | `IDLE_RATE_LIMIT` 300s (dir/events.rs:1002) | dir/events.rs:2885 → dir/prompts.rs:1483, :1534, :1867-1919; gate app/merge_alert.rs:87-139; send recheck dir/prompts.rs:1247, :1290; retract rt/teams.rs:1618 | "⚠️ MERGE REQUIRED — supervisor action needed" / "WAITING ON INDEPENDENT QA" / "idle while task T still S, close rejected" | supervisor | RE-POLL CI + target ref (app/merge_alert.rs:139, :593-633) | receipt per (task, tip) (app/merge_alert.rs:39-42); once per (task, tip, ci_state) (dir/events.rs:1881-1890) | N-24 (park relay), W-03 MergeBranches | cas-4175 | MERGE(N-24): announced twice per park |
| W-49 | Director lane-CI evidence (CiStatus) | each refresh; 10s budget (rt/factory_jobs.rs:38) | rt/director_refresh.rs:827; app/delivery_evidence.rs:416-431 | internal input to W-03, W-47, W-48 | internal | RE-POLL git + gh (60s cache only for "pending") | cache | W-42, W-43 | none recorded | MERGE(W-42): one CI poller writing CI facts |
| W-50 | `ci_completed` reminder | same CI poll | rt/ci_watch.rs:225 → qe:13478 | fires a user-set reminder | reminder target | RE-POLL GitHub runs for SHA | `mark_fired` (qe:13583) | none | none recorded | KEEP: user-requested condition |
| W-51 | External-wake scan (branch contained / tag exists) | 60s (rt/ci_watch.rs:24); backoff to 30 min (rt/external_wake.rs:25); expiry 24h (:31) | rt/external_wake.rs:363; `git fetch` rt/ci_watch.rs:747, `ls-remote` :761 | fires a user-set reminder | reminder target | RE-POLL origin refs (fetch per ref) | pending reminder row + `mark_fired` | W-45 (same "merged into") | none recorded | KEEP: user-requested; should read PROJ-delivery when the ref is a delivery |
| W-52 | Release-train cut notice | same poll | lcy:4526-4534 → cas-cli/src/train_guardrails.rs:352 | "train ready to cut" | supervisor | DERIVED from that poll's PR read | once per train state (lcy:4526) | W-03 ReleaseVerdictDue | none recorded | KEEP |
| W-53 | Train CI observe / merge resume / CI retry | same poll | lcy:4492-4507; cas-cli/src/factory_ci_retry.rs:412 | auto-acts on durable merge intents; writes "Automatic CI merge retry completed" task note | none / task notes | RE-POLL GitHub + store | intent rows | W-42 | note repeated in cas-1f65 and cas-c1fb notes | KEEP |
| W-54 | Violet v1 claim/ack poll | 15s (rt/violet_activity.rs:47; lcy:4601) | rt/violet_activity.rs:1840 | "Slack activity in #ch" | supervisor | RE-POLL Cloud relay | envelope `dedupe_key`; 1 wake/channel/60s (:54); 600s hold (:57) | W-55 | none recorded | MERGE(W-55): v1 is suppressed once v2 answers |
| W-55 | Violet v2 managed reader | same tick | rt/violet_reader.rs:691, :726 | delivery / mention (urgent) / recovery notice | supervisor | RE-POLL Cloud | `violet-delivery:` / `violet-recovery:{delivery_id}` (:735) | W-54 | none recorded | KEEP |
| W-56 | Violet channel watch sweep / idle stop | 300s sweep (rt/violet_activity.rs:49); 3600s idle stop (:51) | rt/violet_activity.rs:2297, :1206, :1867 | wake, or inbox-only "watch stopped" | supervisor | RE-POLL `violet_read` | seen_keys + dedupe key | W-54 | none recorded | KEEP |
| W-57 | Violet watch health | on status/doctor; 300s stalled claim (:1479); 24h read error (:79) | rt/violet_activity.rs:1640-1670 | warning line | operator via `cas doctor` (pull) | DERIVED watch book | none (pull) | none | none recorded | KEEP |
| W-58 | Daemon loop wedge watchdog | OS thread every 250ms (cas-cli/src/factory_daemon_health.rs:40); pass wedged after 1s / 60s (:30); re-report every 30s (rt/loop_watchdog.rs:1176) | rt/loop_watchdog.rs:1185-1335 (wedge check :828) | writes `loop.json` (wedge lane/step/wchan), TUI badge, WARN log | worker_status readers, TUI | RE-POLL in-process atomics + thread wchan | badge ≤1/s (:64); re-report 30s | W-18 | GH #1363 (37-min silent wedge) | KEEP: real liveness loss, no other signal (PROJ-liveness) |
| W-59 | Wedged-shutdown forced exit + hung helper kill | same thread; exit 20s after shutdown while wedged (rt/loop_watchdog.rs:1172); helpers ≥30s old when reset pending (:47) | rt/loop_watchdog.rs:1236-1253 (exit 70), :1291-1295, :1140-1163 | records `forced_exit` lifecycle; kills hung git/gh/ssh children | daemon lifecycle log | RE-POLL `ps` | one-shot | none | GH #1363 | KEEP |
| W-60 | PTY write stalled (`pty_write_stalled`) | when one physical write passes its outcome bound | rt/inject_commands.rs:183 → lcy:1545-1560 | blocked PTY write detail | supervisor | RE-POLL PTY write outcome | occurrence key | W-27 | cas-bdbc | KEEP |
| W-61 | MCP server parent watchdog | poll every 5s (cas-cli/src/mcp/server/parent_watchdog.rs:89); 2 confirmations (:93); 10s grace (:98) | parent_watchdog.rs:227-249 | "[Cassy] Parent harness is gone (ppid A -> B); shutting down" then exits | the process (stderr) | RE-POLL ppid and /proc | one-shot | none | none recorded | KEEP: real liveness loss |
| W-62 | GH merge-queue watchdog workflow | cron `*/5 * * * *` (.github/workflows/merge-queue-watchdog.yml:5) | scripts/cancel-stale-merge-group-runs.sh:14-49; policy scripts/watchdog-policy.sh:14 | cancels `merge_group` runs whose archive job ran longer than 1200s | GitHub (job log) | RE-POLL GitHub API | re-reads status before cancel (:36-46) | W-44 | none recorded | KEEP: CI hygiene; tag cancellations so W-44 does not report them as ejections |
| W-63 | GH stale queued-run watchdog | cron `*/5` (.github/workflows/stale-queued-run-watchdog.yml:5) | scripts/cancel-stale-non-merge-group-queued-runs.sh:16-51 | cancels non-`merge_group` runs queued longer than 1200s | GitHub | RE-POLL GitHub API | status re-read (:36) | W-62 (split on purpose) | none recorded | KEEP |
| W-64 | systemd install-on-publish | `OnBootSec=2min`, `OnUnitActiveSec=60s` (ops/systemd/cassy-install-on-publish.timer:6-7) | ops/systemd/cassy-install-on-publish.service:12 | installs a newly published release | host | RE-POLL release feed | none visible | none | none recorded | KEEP |
| W-65 | systemd actions cache prune | daily 04:15 + ≤15 min jitter (ops/systemd/cassy-actions-cache-prune.timer:6-8) | ops/systemd/cassy-actions-cache-prune.service:11 | prunes caches when runners are idle | host | disk + runner state | none | none | none recorded | KEEP |
| W-66 | systemd runner restart | `Restart=always` (ops/systemd/cassy-actions-runner*.service:15/17) | ops/systemd/cassy-actions-runner*.service | restarts dead runners | host | process table | systemd backoff | none | none recorded | KEEP |
| W-67 | External-work reconcile | every 2s | rt/external_work_reconcile_jobs.rs:14 | silent auto-act against Cloud and Violet | none | RE-POLL Cloud/Violet | none | none | none recorded | KEEP: silent reconcile, no notice |
| W-68 | Commander work-events change probe | every 2s for subscribers (rt/work_events.rs:1739) | rt/work_events.rs:1739 | pushes changed views to Commander | Commander | DERIVED store (re-reads each 2s) | change fingerprint | N-35 | none recorded | KEEP: already a derived projection feed; the natural home for PROJ-* |
| W-69 | Unread-message operator alert | emitted with each W-31 sender bounce | rt/poison_sweep_jobs.rs:314, :353 | "Message from A to B unread" (operator alert) | operator | DERIVED prompt_queue | `delivery-stalled-operator:{bounce_id}` (rt/poison_sweep_jobs.rs:339) | W-31, W-34 | none recorded | MERGE(W-34): one operator escalation per lost message |

</details>

<details open data-collapse>
<summary>D. All 105 notices</summary>

Table: Notices N-01 to N-105

| ID | Name | Trigger and cadence | Source file:line @01df2dc77 | Condition detected / what it says | Recipient | Source it reads | Dedup/rate-limit | Duplicates | Known FPs / floods | Verdict |
|---|---|---|---|---|---|---|---|---|---|---|
| N-01 | Train-freeze held (`progress_operator_held`) | W-01 sweep, when parked deliveries target a frozen train | pw:131-152 | "Parked for a frozen release train, so not a stall: ids… Only the operator can lift it" | supervisor | DERIVED freeze label/record | in-memory `HELD_REPORTED` (pw:22) + `train-freeze-held:{train}:{at}` (pw:135) | Commander train card | added by cas-1f65 | MERGE(W-03): an owed action whose owner is the operator; emit at freeze time |
| N-02 | TaskAssigned | assignee change in snapshot | dir/events.rs:2363; dir/prompts.rs:1677-1705 | "You have been assigned a new task…" | worker | DERIVED task DB | once per (task, assignee) (dir/events.rs:2349-2357) | N-27 | none recorded | KEEP |
| N-03 | TaskCompleted, lease regressed | task left active set back to Open | dir/events.rs:2445; dir/prompts.rs:1751-1761 | "…it is now Open (lease may have expired)… ask the worker to close" | supervisor | DERIVED | once per task (dir/events.rs:2444) | N-25 | none recorded | KEEP |
| N-04 | AgentRegistered | new active agent not yet messaged | dir/events.rs:2919-2939; dir/prompts.rs:2154-2167 | "Worker X has registered and is awaiting its first task" | supervisor | DERIVED | 30s debounce; dropped on assignment | W-09, W-03 AssignReadyWork | none recorded | MERGE(W-03) |
| N-05 | EpicAllSubtasksClosed | active subtasks drop to 0 | dir/events.rs:3074; dir/prompts.rs:2193 | "epic is ready to close" | supervisor/owner | DERIVED (+ `epic_completion_is_current`) | 30s debounce | W-03 AssembleGatePipeline | none recorded | MERGE(W-03) |
| N-06 | Directed-message burst wake | `created_at + debounce` (default 20s, cap 300; cas-cli/src/config/settings.rs:1310); ask/answer/blocker immediate | cas-cli/src/supervisor_message_wakes.rs:321-327, park :348 | batches peer/worker messages into one supervisor turn | supervisor | DERIVED prompt_queue + wake ledger | one ledger row per lead message (:190-210); pure acks wait 1h (:343-346) | W-31 | cas-1e7e2 (origin), cas-2961 (acks) | KEEP: PROJ-message wake policy |
| N-07 | Wake-cap notice | sender→supervisor wakes/hour reach cap (default 6, settings.rs:1312) | supervisor_message_wakes.rs:297-314 (count :246-249) | "Wake cap: messages from S to SUP reached N wakes/hour. Batched…" | supervisor | DERIVED ledger | `supervisor-wake-cap:{sender}:{sup}:{hour}`; recheck 1 min (:335) | none | none recorded | KEEP |
| N-08 | Worker died (pane exit / `worker_died`) | event: `PaneExited` (qe:5917) | qe:5957-6089 `handle_worker_crash`; relay orph:546; desktop cas-cli/src/ui/factory/notification.rs:172 | stale mark with Deferred/Requested relay; "Worker process … before Cassy agent registration" (qe:6037) | supervisor + desktop | DERIVED PTY exit + agent store | spawn-boot-exit key (qe:6046-6049); incident key (orph:802-808) | W-12, W-19, W-22 | cas-c74f8 path (shutdown during boot) | KEEP: PROJ-liveness event source |
| N-09 | `worker_unavailable` relay helper (unused) | no production caller | lcy:465-489 (callers only tests lcy:2399-2400) | "Worker W's harness reported a terminal unavailable state…" | supervisor | n/a | occurrence key | W-24 | n/a | DELETE: dead code; W-24 owns the condition |
| N-10 | Close-path verification dispatch creation | worker/supervisor close needing verification; deadline 600s (co:87) | co:97, called co:10491 (`close_task_with_dispositions_inner` co:7705) | inserts pending `verification_dispatches` row; `Requested` / `AlreadyActive` | none (DB write) | DERIVED task + git proof boundary | partial unique index one active per task (crates/cas-store/src/verification_store.rs:246; m211:26); same boundary reuses row (verification_store.rs:1149-1166) | N-11, N-12, N-13, W-40 | **cas-b6a3** (cross-process create race → unique-index error), cas-3448, cas-0101 | KEEP: canonical creator; key the dispatch on (task, SHA) |
| N-11 | Receipt-path dispatch | worker `submit_worker_completion_receipt` | co:5804; atomic insert crates/cas-store/src/delivery_store.rs:578, :689; reuse co:6159 | receipt + delivery + dispatch in one transaction | none | DERIVED | same index; exact replay returns existing (delivery_store.rs:672-679) | N-10 (boundary mismatch → conflict) | none recorded | KEEP: atomic; shares N-10's key |
| N-12 | Premerge candidate dispatch | supervisor `worktree_merge` | cas-cli/src/mcp/tools/core/workflow/premerge_verification.rs:1010, :1197 | dispatch bound to `task_at(candidate.proof)` + "needs approval / spawn task-verifier" refusal | supervisor (tool error) | RE-POLL git candidate | same index; rebind if patch unchanged (verification_store.rs:1593) | N-10 | none recorded | MERGE(N-10): one creation API keyed (task, SHA) |
| N-13 | Supervisor recovery re-dispatch | manual recovery | cas-cli/src/mcp/tools/core/workflow/verification_tools.rs:1463, :1530-1532 | times out the active dispatch and creates a new one for the same task + SHA | supervisor | RE-POLL git re-capture | **none across cycles**: a new dispatch id gets a new announce key | N-10, W-39 | duplicate announcements of the same task+SHA | MERGE(N-10) |
| N-14 | VERIFICATION REQUIRED handoff | after N-10 creates a dispatch, factory-worker closes only | sp:1447 (enqueue :1483); called co:10541 | dispatch envelope (id, owner, deadline, bound head) | supervisor | DERIVED dispatch row | `verification-dispatch:{dispatch_id}` (sp:1433, :1480) | N-15, N-17, N-18 | re-announced on every recovery/re-park (key is per dispatch, not task+SHA) | KEEP: rekey to (task, SHA) |
| N-15 | Candidate verification handoff / automatic verifier launch | after N-11 (co:6255) and N-12 (premerge_verification.rs:1119, :1207) | premerge_verification.rs:905, :919 → sp:1498 (enqueue :1515; operator fallback :1511) | "Candidate verification required" | supervisor, else operator | DERIVED | `verification-dispatch:{dispatch_id}` (sp:1514) | N-14 | none recorded | MERGE(N-14) |
| N-16 | Automatic verifier could not start | auto-verifier launch fails | cas-cli/src/mcp/tools/core/workflow/premerge_verification/automatic.rs:370 | "Task T dispatch D: Automatic verifier could not start…" | supervisor/operator | DERIVED | `supervisor-notice:premerge_verification:{dispatch}:{detail}` (cas-cli/src/supervisor_notices.rs:101) | none | none recorded | KEEP |
| N-17 | VERIFICATION REQUIRED gate text | synchronous close refusal | co:8124, co:10703; lc/close_ops/gate_text.rs:340; cas-cli/src/mcp/tools/core/task/update.rs:2123 | refusal naming the pending dispatch | caller (tool response) | DERIVED | n/a (synchronous) | N-14 | none recorded | KEEP |
| N-18 | `worker_verification_blocked` activity | every gated close | co:10473 | "Awaiting verification: T" over the daemon socket | supervisor UI | DERIVED | none | N-14 | none recorded | MERGE(N-14) |
| N-19 | VERIFICATION TIER task note | on close | lc/review_tier.rs:41; co:9796-9798 | "🧭 VERIFICATION TIER n at tip: …" | task notes | RE-POLL git diff paths | `notes.contains(note)` (co:9796) | none | repeated 3× in cas-1f65 and cas-c1fb notes | KEEP: fix the dedup (contains-check misses the variant) |
| N-20 | Approved verdict, close recovery | approved verdict on an unfinished close | sp:405, :443; called verification_tools.rs:815, :1214 | "Verification approved… run task close" | supervisor/operator | DERIVED | `supervisor-notice:verification_close_recovery:{dispatch}:{recipient}` | W-41 (escalates after 5 min) | none recorded | KEEP |
| N-21 | Independent QA round handoff | QA round opens | sp:1533, :1562; called lc/qa_dispatch.rs:1696 | "Independent QA required" | supervisor | DERIVED qa_passes | `qa-dispatch:{pass_id}`; unique active pass (crates/cas-store/src/qa_pass_store.rs:45) | none | none recorded | KEEP |
| N-22 | QA round superseded | round retired | lc/qa_dispatch.rs:1169, :1194 | "QA round superseded: stop" | reviewer | DERIVED | `qa-dispatch:superseded:{pass}` | none | none recorded | KEEP |
| N-23 | QA rejection escalation | failed-round limit (lc/qa_dispatch.rs:973) | lc/qa_dispatch.rs:1784, :1809 | "Independent QA rejected T n times… decide" | supervisor | DERIVED | `qa-dispatch:escalate:{pass}` | none | none recorded | KEEP |
| N-24 | Park relay (MERGE REQUIRED, `task_awaiting_merge`) | at park (co:6528) and re-park (co:7328) | sp:1086 (`notify_idempotent` sp:1158) | high-priority lifecycle wake | owning supervisor | DERIVED task DB | `transition_key` = task:old:new:session:kind:updated_at (sp:138); **every re-park makes a new key** | W-48, W-34, W-03 | cas-20ac (relay flood) | KEEP: PROJ-delivery survivor; rekey to (task, SHA, parked) |
| N-25 | Other lifecycle pushes (Closed / ReadyReopened / Started / Blocked / Ready) | each transition: co:12139, :12323, :13169, :13252; lc/../lifecycle.rs:3717; update.rs:2770 | sp:1086 | FYI or blocked wake | supervisor | DERIVED | `transition_key` | W-41 | none recorded | KEEP |
| N-26 | Worker lifecycle notices (merge-closed, merged-lane park, rejected, conflict, source fix, request_changes, no-delivery, close over hold) | the matching event | sp:456, :468, :499, :520, :539, :559, :581, :612 | quiet or next-turn worker notices | assigned worker | DERIVED | per kind, e.g. `cassy-merge-closed:{task}:worker:{w}` (sp:481), `cassy-merge-conflicted:…` (sp:548) | W-45 | none recorded | KEEP |
| N-27 | Auto-unblock worker wake | a blocker closes (co:12274) | sp:335, :369 (300s stale-assignee window sp:219) | "Task unblocked: run task start" | worker | DERIVED | `{base}:worker:{name}` (sp:356) + note marker | N-02 | none recorded | KEEP |
| N-28 | Second merge-queue ejection page (P0) | 2nd failed run for the same head (W-44) | lcy:1060-1070 | "P0: T ejected twice from the merge queue… stays parked until you decide" | operator | DERIVED task notes + run id | `merge-queue-second-ejection:{task}:{run}` | W-44 | none recorded | KEEP: an operator-owned decision fact |
| N-29 | `supervisor_notices` operator fallback + adoption | no live supervisor; adopted on operator-reply read (qe:3029) | cas-cli/src/supervisor_notices.rs:118, :163; producers cas-cli/src/train_guardrails.rs:208, supervisor_notices.rs:246, lcy:874, sp:1511, cas-cli/src/factory_binary_skew.rs:397-407 | holds owner notices for the operator | operator | DERIVED supervisor_queue | `supervisor-notice-operator:{id}` (:184) | none | none recorded | KEEP |
| N-30 | Fast-lane QA walk ask | failed hotfix walk | cas-cli/src/fast_lane_walk.rs:164 | ask with Revert / Fix-forward options | operator | DERIVED | `fast-lane-walk:{task}:{merge}` | none | none recorded | KEEP |
| N-31 | Agent message `target=operator` | agent-initiated | cas-cli/src/mcp/tools/service/agent_search_system/message.rs:1950 | free text, kind, attachments | operator | none | **none** | none | none recorded | KEEP: add a dedup key |
| N-32 | CLI factory message to operator | operator CLI | cas-cli/src/cli/factory/queries.rs:849 | free text | operator | none | none | none | none recorded | KEEP: user-initiated |
| N-33 | Operator delivery outbox routing | any `enqueue_idempotent(target="operator")`; `record_operator_turn` | crates/cas-store/src/prompt_queue_store.rs:6894-6906 | routes into `operator_delivery_outbox` | operator (Commander, desktop) | none | caller's key | all operator rows | none recorded | KEEP: the single operator channel |
| N-34 | Commander supervisor-turn mirror | each supervisor turn | rt/commander_mirror.rs:1036 → prompt_queue_store.rs:9513 | mirrored supervisor turn | Commander | RE-READ transcript | `commander-mirror:{session}:{agent}:{turn}` | none | none recorded | KEEP |
| N-35 | Director events to Cloud | every applied refresh (lcy:5590) | rt/cloud.rs:93 | raw director events and state | Cloud / Commander | DERIVED snapshot | director 30s debounce only (dir/events.rs:997) | W-03, W-47, W-48 | none recorded | KEEP: publish PROJ-* instead of raw events |
| N-36 | Task-note context blocks (START CONTEXT close checks, test principle, Predicted QA impact, finding repro) | at task start / show | lc/close_ops/gate_preflight.rs:152; cas-cli/src/task_assignment.rs:8; lc/qa_impact.rs:100; sync merge suspected in cas-cli/src/cloud/syncer/pull.rs (not traced) | close-check list, "Elegant tests…", QA-impact table, Repro/Affected | worker (task notes) | RE-COMPUTE QA impact (CODEMAP staleness) | none observed | N-66 (CODEMAP stale warning inside QA impact) | the Repro + QA-impact block repeats after every note: cas-098dd ×5, cas-1f65 ×9, cas-c1fb ×9 | MERGE(N-17): render at show/start time; do not persist into notes |
| N-37 | CAS Context header | SessionStart, every start | bs:229 (text cas-core hooks/context/mod.rs:524) | "use the Cassy task, memory and search tools" | any | constant | resume delta only (hs:670-741) | none | none | KEEP |
| N-38 | Session id line | SessionStart (MCP enabled) | bs:234 | "**Session:** `<id>`" | any | hook input | none | none | none | KEEP |
| N-39 | Blocked Tasks (Action Required) | SessionStart, blocked in-progress tasks | bs:250 | lists blocked tasks | any | DERIVED task DB | resume delta | N-97 | none | KEEP |
| N-40 | Agent Coordination + role guidance | SessionStart, factory or >1 agent | bs:318; coordination.rs:88, :260 | roster + static supervisor/worker guidance | worker, supervisor | DERIVED agent DB + static | resume delta | N-81 | none | KEEP |
| N-41 | Pinned Memories | SessionStart, if pinned | bs:360 | pinned memory bodies | any | DERIVED memory DB | resume delta | none | none | KEEP |
| N-42 | Current Handoff (role) | SessionStart, if handoff exists | bs:407-440 | newest handoff, truncated | any | DERIVED memory DB | resume delta | N-55, N-56 | none | KEEP |
| N-43 | In Progress / Ready Tasks | SessionStart (non-minimal) | bs:469, :534 | task listings | any | DERIVED task DB | resume delta | N-103 | none | KEEP |
| N-44 | Critical / Active Rules | SessionStart | bs:599, :657 | rule listings | any | DERIVED rule DB + cwd glob | resume delta | N-103 | none | KEEP |
| N-45 | Available Skills | SessionStart | bs:748 | skill index | any | DERIVED | resume delta | none | none | KEEP |
| N-46 | Memories (pointer) / Helpful Memories / Related to Current Work | SessionStart | bs:797 (text :173), :910, :1000 | pointer or ranked memories | any | RE-COMPUTE search ranking | resume delta | N-93 | none | KEEP |
| N-47 | Project Knowledge | SessionStart, if pages | bs:122 | knowledge page list | any | DERIVED | resume delta | none | none | KEEP |
| N-48 | More Context Available + token footer | SessionStart | bs:1033, :1075 | counts + "**Context: Ntk**" | any | computed | none | none | none | KEEP |
| N-49 | Host Constraints | SessionStart, `host:<hostname>` memories | cas-cli/src/hooks/context.rs:271 (text :419) | host-scoped constraints | any | DERIVED global store | resume delta | N-76 | none | KEEP |
| N-50 | Connected MCP Tools | SessionStart, proxy catalog cache | context.rs:282 (text :1135) | server/tool list | any | DERIVED cache file | resume delta | none | none | KEEP |
| N-51 | Personal Patterns (Cloud) | SessionStart, logged in | context.rs:293 (text :940) | Cloud patterns | any | RE-POLL Cloud `/api/patterns`, 5s timeout | none | none | adds up to 5s to each start | KEEP |
| N-52 | Team Suggestions (Cloud) | SessionStart, team set | context.rs:299 (text :1049) | team suggestions | any | RE-POLL Cloud | none | none | adds start latency | KEEP |
| N-53 | AI-selected context | SessionStart, `hooks.ai_context` | context.rs:238, :570, :582 (spawns `claude` :876) | model-chosen context | any | RE-POLL model call | none | replaces N-37..N-48 | none | KEEP |
| N-54 | Plan-mode context | SessionStart, permission_mode=plan | hs:234; cas-core plan_mode.rs:18 | planning context | any | DERIVED | none | replaces base | none | KEEP |
| N-55 | Supervisor lane handoff | SessionStart supervisor, unseen handoff/recovery row | hs:279; cas-cli/src/hooks/supervisor_continuity.rs:230-318 | predecessor receipt, rerouted messages, adopt owned epics, recovery list | supervisor | RE-POLL git integration scan + DERIVED queue | idempotent key + surface-unseen limit 1 (:284, :318) | N-42 | none | KEEP: the durable receipt; the memory handoff (N-42) is the summary |
| N-56 | Messages newer than this handoff | SessionStart, handoff names Violet targets | hs:275; handoff_updates.rs:19 (4s budget :13) | Slack posts newer than the handoff (≤1800B) | any | RE-POLL Violet/Cloud | none | N-42 | none | KEEP |
| N-57 | Agent role mismatch | SessionStart, env role ≠ registered row | hs:283 (text :17) | "Cassy AGENT ROLE MISMATCH… run whoami and `cas doctor`" | factory agents | DERIVED agent DB | none | none | none | KEEP |
| N-58 | Viktor inbound | SessionStart supervisor, pending rows | hs:293; cas-cli/src/mcp/viktor_watch.rs:119-147 | `<viktor-inbound>` bodies | supervisor | DERIVED inbound DB | marks rows surfaced (viktor_watch.rs:125) | none | none | KEEP |
| N-59 | Viktor upstream absent (SessionStart) | every start while unhealthy | hs:298; viktor_watch.rs:150-185 (text :182) | "⚠ Viktor upstream absent… Restore VIKTOR_API_KEY" | any | DERIVED health cache + watch DB | none | N-60 | repeats every start | KEEP: derived from the stored health state |
| N-60 | Viktor upstream absent (daemon queue) | daemon health change | viktor_watch.rs:103 | "Viktor upstream absent — N watched run(s) unpollable" | supervisor | DERIVED | not traced | N-59 | none | MERGE(N-59) |
| N-61 | Factory MCP binary skew | SessionStart supervisor (renders even when clean); post-install notice | hs:315; cas-cli/src/factory_binary_skew.rs:345, `notify_after_install` :397-407 | "Factory MCP versions (installed X)" / "⚠ … older than installed" | supervisor | RE-POLL agent DB + daemon epochs (120s) | none | N-29 | renders a table when nothing is skewed | KEEP: emit only on skew |
| N-62 | Concurrent supervisors (local) | SessionStart supervisor | hs:325 (text :756) | "CONCURRENT SUPERVISORS ACTIVE: names" | supervisor | DERIVED local agent DB | none | N-64 | none | MERGE(N-64): one peers block |
| N-63 | Configured Slack channels | SessionStart, `slack.channels` set | hs:335; cas-cli/src/cli/violet_cmd/channels.rs:415 | per-channel state (stale after 24h) | any | DERIVED snapshot file | none | W-57 | none | KEEP |
| N-64 | Live peer supervisors (Cloud) | SessionStart supervisor | hs:355; cas-cli/src/cloud/peers_session_start.rs:31, :106 (3s :16) | heartbeat, focus, claims, overlaps | supervisor | RE-POLL Cloud | none | N-62 | none | KEEP |
| N-65 | Backlog hygiene | SessionStart supervisor, report not clean | hs:374; cas-cli/src/hooks/backlog_hygiene.rs:829, :873 | open tasks already delivered on default branch; epics with all children ended; idle 2 weeks | supervisor | RE-POLL git log/grep on default branch | none | W-45, N-05 | none | KEEP: detects store-versus-git drift; becomes unnecessary once PROJ-delivery is authoritative |
| N-66 | Codemap freshness | SessionStart, missing/stale (prepended if severe) | hs:393, :400; cas-cli/src/hooks/handlers/handlers_events/codemap.rs:266, :289; docs_freshness.rs:158 | "CODEMAP.md is out of date / has N changes… Run /codemap" | any | RE-POLL git diff | none | N-99, N-36 | appears in every task's QA-impact block (cas-098dd, cas-1f65, cas-c1fb) | KEEP |
| N-67 | Project-overview freshness | SessionStart, missing/stale | hs:415; cas-cli/src/hooks/project_overview.rs:494 | same shape for PRODUCT_OVERVIEW.md | any | RE-POLL git | none | N-36 | PRODUCT_OVERVIEW watch-target warning repeats in QA impact | KEEP |
| N-68 | Prior-factory WIP | SessionStart supervisor, dirty main worktree | hs:438; sh:588 | dirty files + last-touching task; "Triage BEFORE spawning workers" | supervisor | RE-POLL git status + log | none | none | none | KEEP |
| N-69 | Stale builtin references | SessionStart, locally edited refs skipped | hs:456; sh:681, :727 | "N builtin skill reference file(s) are NOT being updated" | any | DERIVED sync ledger | none | none | none | KEEP |
| N-70 | Skill sync blocked | SessionStart, managed-file conflicts | hs:470; sh:693 | "Cassy preserved N conflicting managed file(s)" | any | DERIVED conflicts file | none | none | none | KEEP |
| N-71 | Unfiled reports | SessionStart, staged BUG-/FEATURE- reports | hs:494; sh:766, :872 | "N staged report(s) were never filed to GitHub" | any | RE-POLL dirs | none | N-72 | none | KEEP |
| N-72 | Issue repository target | SessionStart, docs/requests/ exists and `[issues] repo` unset | hs:510; sh:903, :927 | "`[issues] repo` is unset… Do not derive it from origin" | any | DERIVED config + dir check | none | N-71 | none | KEEP |
| N-73 | Orphaned factory resources | SessionStart supervisor | hs:529; sh:939, :970 | "Leftovers from earlier sessions: N orphan process(es)…" + gc commands | supervisor | RE-POLL process + worktree scan | none | W-19 | none | KEEP |
| N-74 | GitHub issue triage | SessionStart supervisor, `issues.repo` set | hs:547; cas-cli/src/hooks/issue_triage.rs:67, :85, :191 | "## GitHub issue triage — repo, N open…" | supervisor | RE-POLL `gh api graphql` (1s, :16) | 5-min cache (:15) | none | none | KEEP |
| N-75 | Integrations freshness | SessionStart, opt-in | hs:562 (text :1586) | `<integrations-freshness>` | any | RE-POLL integrate doctor | none | none | none | KEEP |
| N-76 | Large-artifact staging | SessionStart, `staging.staging_dir` set | hs:570 (text :775) | "Stage large artifacts (>1GB) in DIR — /tmp is tmpfs" | any | config | none | N-49, N-95 | none | KEEP |
| N-77 | Codex finish hint | SessionStart, Codex | hs:582 | "type `$finish`…" | Codex agents | constant | none | none | none | KEEP |
| N-78 | Worker worktree alert | SessionStart isolated worker, wrong cwd/branch | hs:602 (text :1622-1690) | CWD MISMATCH / DETACHED HEAD / WRONG BRANCH | worker | RE-POLL git | none | PreToolUse denies | none | KEEP |
| N-79 | Project hold | SessionStart, hold active/unreadable | hs:605; crates/cas-store/src/project_hold.rs:54 | "Project hold: reason / operator… only the operator can lift" | any | DERIVED hold file | none | none | none | KEEP |
| N-80 | Resume unchanged | SessionStart resume, base unchanged | hs:736 | "base context unchanged; previously delivered guidance still applies" | any | fingerprint file | is the dedup | none | none | KEEP |
| N-81 | Supervisor reminder (per prompt) | UserPromptSubmit, every supervisor prompt | cas-cli/src/hooks/handlers/handlers_middle/prompt_capture.rs:140 (text :19-29) | "[supervisor reminder] You are X… AskUserQuestion BLOCKED; never SendMessage/close/implement; never poll" | supervisor | env only | **none** (every prompt) | N-40; PreToolUse deny enforces the rule | per-prompt repetition | DELETE: enforced by PreToolUse deny; N-40 carries the text |
| N-82 | Inbox turn-start block | UserPromptSubmit, unread rows | prompt_capture.rs:143; fi:297, :449, :770, :1103 | "[incoming messages] …arrived while you were not in a turn… act on them now" | worker, supervisor | DERIVED queue + RE-CHECK stale assignment (fi:472) | per-row receipts copied to aliases (fi:704); 9500-char budget (fi:342) | N-90, daemon PTY copy, MCP response append (cas-cli/src/mcp/tools/service/server_handler.rs:785) | same row can appear 3 ways | KEEP: PROJ-message surfacing; one receipt per row |
| N-83 | [digest] notices | first surfacing in a turn | fi:610-650 (text :191) | "[digest] N inbox updates: #id summary…" | factory agents | DERIVED NoticeDigest | receipts (fi:644) | none | none | KEEP |
| N-84 | [already shown] | turn start, prompt names a surfaced row | fi:671 (text :1065) | "pointer or held copy, not new" | factory agents | DERIVED receipts | turn-start only | N-82 | none | KEEP: itself a dedup marker |
| N-85 | [withdrawn] | turn start, stale assignment | fi:678 (text :827) | "task no longer open for you" | worker | DERIVED task DB | turn-start only | none | none | KEEP |
| N-86 | [not shown here] + Pending list | surfacing overflow | fi:679 (text :862, :870) | rows left unread; retried | factory agents | DERIVED queue | retried until complete | N-87 | none | KEEP |
| N-87 | [sent in full] | turn start, Teams body fallback | fi:695 (text :841) | "sent in full as its own teammate message" | factory agents | DERIVED | turn-start only | W-30 | none | KEEP |
| N-88 | [send order] | turn start, operator typed while Commander rows pending | fi:747 (text :799) | "answer these first, oldest first" | supervisor | DERIVED | none | none | none | KEEP |
| N-89 | Auto-detected preference | UserPromptSubmit non-supervisor, preference-like prompt | prompt_capture.rs:312-315 | "Auto-detected preference… Created rule X" | any non-supervisor | creates a draft rule | **none** (rule each time) | none | none recorded | KEEP: add dedup on rule text |
| N-90 | Inbox tool-boundary block | PostToolUse, every tool call (stamp read; full drain when newer) | cas-cli/src/hooks/handlers/handlers_middle/post_tool.rs:23-24; cas-cli/src/hooks/turn_context.rs:409-441; header fi:451 | "[incoming messages] …arrived while you were working…" | factory agents | DERIVED queue + transcript tail (256KB) / deep scan ≤64MB (turn_context.rs:22) | `mail_checked_at` cursor (turn_context.rs:410), shared receipts, file lock | N-82 | none recorded | KEEP: same projection and receipts as N-82 |
| N-91 | CAS-wake pointer rows | first tool boundary of a "CAS wake: message N" turn | turn_context.rs:375-395; fi:453 | "…the ones this turn's CAS wake names…" | factory agents | DERIVED queue + transcript | `pointers_resolved` (turn_context.rs:391) | N-82 | none | KEEP |
| N-92 | Polled-unacked repeat | first tool boundary per turn | turn_context.rs:400-407; fi:447, :813 | "[not acknowledged] one-time repeat of inbox_poll reads" | factory agents | DERIVED | `polled_checked` per turn (:405) | N-82 | none | KEEP |
| N-93 | Operation lessons | PostToolUse on release-cut, spawn, report, epic-close | post_tool.rs:16-17; cas-cli/src/hooks/operation_lessons.rs:65, :81 | "[operation lessons: op] Check each…" ≤6 memories | any | DERIVED memory + task DB | once per memory id per session (:117, :125) | N-46 | none | KEEP |
| N-94 | Ripple check | PostToolUse, every Write/Edit of a file named in an open task or spec | post_tool.rs:101 (text :803) | "Ripple check: `file` is referenced in task(s)/spec(s)… verify consistency" | any | RE-SCAN all Open+InProgress tasks and specs each edit | **none** | none | repeats on every matching edit | DELETE: no dedup, no owner, scans the whole backlog per edit |
| N-95 | tmpfs / ramfs guardrail | PostToolUse Write/Edit/Bash, growth crosses a threshold multiple | post_tool.rs:93; cas-cli/src/hooks/tmpfs_guardrail.rs:77, :274 | "WARNING: session has written/grown tmpfs…" | any | RE-POLL mounts + statfs | once per threshold multiple (:210, :242) | N-76 | none | KEEP |
| N-96 | Shared-file lease | PreToolUse, worker edit of a file another agent holds | cas-cli/src/hooks/handlers/pre_tool.rs:111-112, :145-212; cas-cli/src/cloud/file_leases.rs:322 | "⚠️ SHARED FILE: path is being edited by X…" | worker | RE-POLL Cloud claims (2s, file_leases.rs:22) + `git rev-parse` | **Claude: none** (file_leases.rs:300); Codex: 1 per file per 120s (:29) | Codex message variant | repeats per edit on Claude | KEEP: apply the Codex 120s dedup to Claude |
| N-97 | Exit blockers | Stop, non-factory with `block_exit_on_open` | cas-cli/src/hooks/handlers/stop_flow.rs:89; handlers_state.rs:439 | "⚠️ Cannot exit — remaining work…" (blocks stop) | non-factory agent | DERIVED task + agent DB | `stop_hook_active` brake (stop_flow.rs:51) | N-39 | none | KEEP |
| N-98 | Loop iteration | Stop, active loop | cas-cli/src/hooks/session_stop/mod.rs:129 | "🔄 Loop iteration N/M" + prompt | any | DERIVED loop DB + transcript scan | max iterations | none | none | KEEP |
| N-99 | CODEMAP pending at stop | Stop, every turn end with pending changes | stop_flow.rs:754, :789-800; codemap.rs:557-591 | "Note: CODEMAP.md has N pending structural change(s)" | any | RE-POLL pending file / git diff | **none** | N-66 | every stop | MERGE(N-66) |
| N-100 | No wake source | Stop, supervisor with open work and no reminder | stop_flow.rs:790 (text :832) | "No wake source… set coordination action=remind" | supervisor | DERIVED task + agent + reminder DB | **none** (every such stop) | W-03 | every supervisor stop with parked work | MERGE(W-03): PROJ-owed is the wake source |
| N-101 | Uncovered worker | Stop, supervisor messaged a mid-task worker with no reminder | stop_flow.rs:791 (text :939) | "Uncovered worker: you messaged X… set remind_event" | supervisor | DERIVED queue + task + reminders | per-turn window `supervisor-stop-mark.json` (:877) | N-06 | none | MERGE(N-06): the message-wake policy should wake on the reply |
| N-102 | Verifier binding failures | SubagentStart, task-verifier binding fails | handlers_state.rs:752, :761, :776, :787, :811 | "verification will fail closed" variants | verifier's parent | DERIVED agent DB | none | N-17 | none | KEEP |
| N-103 | PreCompact preservation | every compaction | cas-cli/src/hooks/handlers/handlers_events/notifications.rs:343, :356, :383, :396, :434 | Critical Memories, Active Tasks, Session Decisions, Critical Rules "(preserve across compaction)" | any | DERIVED DBs | none | N-43, N-44 | none | KEEP |
| N-104 | Factory compaction checkpoint | PreCompact, worker with active task | notifications.rs:571, :579 | "You are resuming task X on branch Y…" + "compacting, will resume" to supervisor | worker + supervisor | DERIVED agent DB + `git branch` | none | N-82 (supervisor copy) | none | KEEP |
| N-105 | Desktop / terminal notifications | crash, relay escalation, etc. | cas-cli/src/ui/factory/notification.rs:172 (`notify_crash`); qe:5101 | native / bell / iTerm2 notification | operator desktop | n/a (channel) | caller | N-08, W-34 | none | KEEP: an output channel of N-33 |

</details>

<details open data-collapse>
<summary>C and D. Which watchdog or notice fired in each incident</summary>

Table: Incident map

| Incident | Row that fired | What the store already knew | Fix shape at pin |
|---|---|---|---|
| cas-1f65: overnight stall notices at 4:53 and 5:39 AM ET threatened to page the operator over cas-0f47 / cas-1d99, held by a train freeze | W-01 (W-02 armed) | train freeze (release_freeze label, operator-only lift) | special-case exemption + N-01 (pw:96-105, :131) |
| cas-c1fb: fired twice (#49931, #50607) listing 13 deliveries waiting on soundwave's train; 0 local workers | W-01 (W-02 armed) | delivery anchor contained in target; binding epic split | containment + split exemption (pw:72-90, :168-176) |
| cas-098dd: stall page threatened over finished epic cas-6aef whose close the dead-target-branch guard refused (GH #1747); supervisor marked 3 tasks blocked to stop it | W-01 (W-02 armed) | the refused close and its gate code | in progress: persist the refusal receipt, exclude it (third special case; the task itself asks to generalise) |
| cas-e239: operator-blocked epic / held delivery counted as a stall | W-01 | blocker reason, DECISION HOLD | exemptions (pw:58-78) |
| cas-c74f8: shutdown_workers 3639 on booting sharp-spider-93 relayed as "worker harness failed during boot" (notification 101899) | W-12 → N-08 (`DeathRelay::Alarm`, qe:6739) | the shutdown request row | cas-7076 removes the spawn watch on shutdown (parked delivery f5ce0d16a) |
| cas-cdab (open): one expired dispatch woke the supervisor twice (#33 task_blocked, #34 verification_dispatch_expired) | W-41 + W-39 | dispatch deadline | not fixed; merge into one wake |
| cas-b6a3: concurrent identical close retries raced dispatch creation (unique-index error) | N-10 | the active dispatch | made creation cross-process idempotent |
| cas-20ac: lifecycle relay flood survived ack and starved supervisor progress | N-24 / W-37 / W-34 | relay delivery state | fixed (by title) |
| cas-40ed (GH #1687): repeated delivery-watchdog wakes for an empty worker inbox | W-27 | inbox empty | awaiting merge (by title) |
| cas-37b8, cas-f265: supervisor-unread alarm counted FYI rows and a self-reminder | W-32 | row kind / origin | fixed (by title) |
| cas-7156, cas-058e, cas-0028, cas-9829: worker stall nudges flood during long commands, blind to background jobs; permanent keys hid later episodes | W-05 / W-06 / W-09 | none (liveness) | fixed (by title) |
| cas-8391, cas-4175, cas-7809, cas-7310, cas-728b: supervisor_stalled repeats under operator blockers, on merged branches, for peer-owned deliveries | W-03 / W-47 | blocker owner, merge commit, epic split | fixed (by title) |
| Note-flood pattern visible in all four incident tasks: the Repro + QA-impact block repeats after every note | N-36 | n/a | none |

</details>

<details open data-collapse>
<summary>E. All 41 CI and release checks</summary>

Table: CI and release checks C-01 to C-41

| ID | Gate/check | Where it runs | Source file:line @01df2dc77 | Defect it catches | Duplicates | a677 verdict | Status now | Verdict now |
|---|---|---|---|---|---|---|---|---|
| C-01 | Classify CI diff (class, rust-unaffected, web, journeys, bridge, script-tests) | hosted; every ci.yml event | ci.yml:84-125; .github/actions/classify-required-diff/action.yml:120-175 | none itself; routes work | lane.yml:52 copy | (implicit KEEP) | STILL PRESENT | KEEP |
| C-02 | Lane classify + 20 no-build fast rows | lane, **self-hosted** `cas-lane-light` when `CASSY_LANE_SELF_HOSTED=enabled` (it is) | lane.yml:52-101 (rows at :101) | version literals, release-notes lint, skill limits (a677 §10.4) | C-03, C-05, C-06, C-07 | MOVE to one lane lint job | PARTIAL: one lane job, but on self-hosted | KEEP once (hosted) |
| C-03 | Docs Lint: fast rows + markdownlint + release-note drafts + AGENTS.md/CLAUDE.md size and `cas sync agents-md --check` | hosted; docs-only class on PR, queue, push | ci.yml:128-250 (rows :155, mdlint :157, drafts :184, instruction files :219) | markdown/link drift, Grok 10k truncation | C-02 | MERGE into the lane job | STILL PRESENT (lane.yml:103 copy too) | MERGE |
| C-04 | Runner route (self-hosted vs hosted) | hosted | ci.yml:255-282 | none | — | DELETE self-hosted queue routing | PARTIAL: var disabled since Oct 14; routing code and every `mode != 'self-hosted'` clause remain (ci.yml:777-780, 1343-1346, 1867-1870) | DELETE |
| C-05 | Scoped Validation (fast): script suites, fast rows, runner-lock fixtures, web/bridge tests, change-scoped nextest | lane via ci.yml only when not slim; PR when script tests needed | ci.yml:354-520 (rows :401, nextest :475) | impact-selected Rust/script failures | C-06; C-02 rows; lane.yml:143 copy | MERGE with C-06 | STILL PRESENT ×2 (ci.yml and lane.yml:143-228) | MERGE |
| C-06 | Scoped Validation (factory/PR): fast rows, installer/release fixtures, web, bridge, scoped nextest | lane (not slim); PR to a non-default base | ci.yml:522-760 (rows :588, fixtures :590/:596, nextest :683) | as C-05 | C-05; lane.yml:229 copy | MERGE with C-05 | STILL PRESENT ×2 | MERGE |
| C-07 | Pre-push fast rows (worker and main checkout) | local | scripts/fast-rows-pre-push.sh:99; .githooks/pre-push:5-9; daemon worker hook ui/factory/daemon/runtime/teams.rs:2032-2034 | as C-02 (early feedback) | C-02 | DELETE | STILL PRESENT; grew 57→119 lines. Park no longer reads its receipt (a79bccdb6, Oct 29) | DELETE or make advisory |
| C-08 | Main-push / queue tree dedupe + full-gate receipt reuse | hosted | ci.yml:289-327 → scripts/check-ci-merge-queue-validation.sh (61) | none; saves runs | — | DELETE `cas/full-gate` skip logic | STILL PRESENT, re-implemented as tree-bound receipts (cas-8413, 3981a8ba4, Nov 6) | MERGE into one receipt mechanism |
| C-09 | Fast Validation — preflight: installer fixtures, cargo check, ISA-script test, debug build, terminal restoration, CLI smoke + release-asset compile | hosted (queue var disabled); train, queue, main, tags, schedule | ci.yml:764-1006 (Check :900, ISA :909, Build :915, smoke :968) | compile errors; CLI launch | build again in C-24/C-25 | MERGE into parallel jobs | PARTIAL: web, journeys, tiers, bridge, docs split out; Rust steps still serial. Reruns when the suite is reused (`reuse-source == 'full-gate'`, ci.yml:769). On v4.12's queue run it was the critical path: Check 4.5 min + Build 9.2 min = 15.7 min (`gh api …/runs/42813425045/jobs`) | KEEP compile, DELETE debug build when release build exists |
| C-10 | CI tiers: run script-suite tests + release publication guards | hosted; full-set | ci.yml:1008-1078 (:1042, :1049) | machinery regressions only | release-gate `ci-script-tests` | MOVE to non-required, `scripts/`/`.github/` only | STILL PRESENT, gated on rust-unaffected only | MOVE |
| C-11 | Slack bridge build+test | hosted; bridge paths | ci.yml:1080-1135 | bridge regressions | C-05/C-06 bridge steps | — | STILL PRESENT | KEEP once |
| C-12 | Commander web: typecheck, vitest, build + committed-dist drift, visual QA | hosted; web inputs | ci.yml:1137-1236 (dist drift :1188-1200) | 3.18.1 dist drift (cas-211c) | release.sh rebuilds dist at cut (release.sh:334-337); scripts/regenerate-generated-artifacts.sh after every merge (section C) | KEEP, web-gated | STILL PRESENT | KEEP |
| C-13 | Commander journeys, 6 hosted shards × 2 workers | hosted; web inputs | ci.yml:1238-1328 (run :1281) | cas-7c94, cas-cff2, cas-3c25, cas-8fe2 | QA close-gate journey evidence (qa_evidence_gate.rs) | KEEP; add 3 hosted shards | DONE as 6 shards (cf2211ba2, Oct 14) | KEEP |
| C-14 | Suite archive build | hosted (queue var off) | ci.yml:1330-1532 (:1464) | build of test graph | C-09 | KEEP | STILL PRESENT | KEEP |
| C-15 | Rust archive dependency cache (main maintenance) | hosted; main push after queue proof | ci.yml:1534-1583 | none (cache warm) | — | — | NEW | KEEP if cheap |
| C-16 | Suite shards 1-3 (the only full product-suite execution) + MCP store-untouched check | hosted | ci.yml:1585-1750 (:1662, :1714) | cas-546b, cas-113f, cas-194c, cas-3b42, cas-96ba, cas-aaf5 | train push run vs queue run on the same release tree (see D) | KEEP | STILL PRESENT | KEEP |
| C-17 | Queue flake publication (records recovered failures before a green queue) | hosted; queue + train | ci.yml:1752-1801 | flake visibility | — | (machinery) | NEW | DELETE or advisory |
| C-18 | Full-suite fan-in | hosted | ci.yml:1803-1853 | zero-test guard (cas-b78f) | — | KEEP (zero-test) | STILL PRESENT | KEEP |
| C-19 | Doctests | hosted | ci.yml:1855-1985 (:1951) | no recorded catch (a677 §11 Q4) | — | operator decision | STILL PRESENT, required via fan-in (docs/branch-protection/README.md:13) | operator decision |
| C-20 | `Fast Validation` fan-in (required): scoped suites, PR admission, flake publication, every lane, receipt publish, `cas/full-gate` status | hosted | ci.yml:1987-2197 (status :2186-2193); full-validation.yml:28-36 | none itself | — | KEEP (one required context) | STILL PRESENT | KEEP |
| C-21 | Clippy (advisory) | hosted; main, staging push, schedule | ci.yml:2199-2253 | lints | — | — | STILL PRESENT | KEEP advisory |
| C-22 | macOS Check | hosted macos-26 on main push/tags; `ubuntu-latest` report-only on PRs | ci.yml:2255-2436 (runs-on :2286, check :2406) | cas-efd4 (only verified queue product defect) | macOS prebuild C-25 compiles the same tree | KEEP in queue | CHANGED: removed from the queue (602bd0064, Nov 5); now post-merge only | KEEP; decide queue vs post-merge |
| C-23 | Test Compile Guard; Panic isolation ×2; Build benchmark | hosted; main/staging push or schedule | ci.yml:2438-2508, 2510-2594, 2596-2653 | compile-only/measurement | C-09 | — | STILL PRESENT | KEEP scheduled only |
| C-24 | Release prebuild Linux | hosted; queue entry whose version changed; also every main push as a dependency seed | release-prebuild.yml:63-92 (gate), :94-279 (seed concurrency :106, SEED_ONLY :135) | the one shipped build | C-09 build; main-push seed | DELETE main-push prebuild | PARTIAL: main push now seeds the dependency cache and skips on an exact hit (:117-140) | KEEP queue build; DELETE seed if cache hits |
| C-25 | Release prebuild macOS ARM64 | hosted macos-26 | release-prebuild.yml:281-452 | Darwin release build | C-22 | KEEP one build | STILL PRESENT; it is the merged→published critical path (v4.12 lookup waited 13.2 min) | KEEP |
| C-26 | Release: detect version change on main push to cas-cli/Cargo.toml | hosted | release.yml:27-31, :49-117 | — | release-prebuild :63 | ADD (§10.7 #4) | DONE (20561bfeb, Oct 14) | KEEP |
| C-27 | Release: look up prebuilt (Linux/macOS), fallback build via workflow_call | hosted | release.yml:119-185 | — | — | MERGE prebuild + release build | DONE (adopt by commit/tree id, scripts/find-release-prebuild.sh 398 lines) | KEEP |
| C-28 | Release: ISA audit + launch smoke (Linux); signature + launch smoke (macOS) on packaged assets | hosted | release.yml:187-261 (ISA :212, smoke :220, sig :252, smoke :260) | 3.47.0 AVX-512 (cas-2a16), 3.51.0 unlaunchable (cas-e719) | release-gate `release-binary-isa`, `operator-launch-smoke` rows (release-gate.sh:2351-2354) | MOVE here | DONE here; the gate rows still exist (unused on the release path) | KEEP here, DELETE gate rows |
| C-29 | Create Release (tag + notes + PR-opened timer) | hosted | release.yml:263-379 → scripts/release-publish.sh (126) | — | — | replaces the local publisher | DONE | KEEP |
| C-30 | Install-path proof (macOS + clean Linux) | hosted; dispatched by release.yml:323 | install-path-proof.yml:29-252 | installer breakage | test-cas-install.sh in C-06 | — | STILL PRESENT | KEEP |
| C-31 | Deploy Commander to the hosted hub | hosted | release.yml:381-422 → scripts/deploy-hub-release.py (430) | — | — | — | NEW | KEEP (product) |
| C-32 | Install on publish (60 s systemd/LaunchAgent timer) | local host | scripts/install-on-publish.sh:1-40 (256 lines); ops/systemd/cassy-install-on-publish.timer | — | — | ADD (§10.7 #5) | DONE (20561bfeb) | KEEP |
| C-33 | Merge-queue watchdog + stale queued-run watchdog (cron every 5 min) | hosted | merge-queue-watchdog.yml:13-24; stale-queued-run-watchdog.yml:13-24 | stuck runs | — | DELETE self-hosted orphan watchdogs | STILL PRESENT | DELETE if hosted-only |
| C-34 | `scripts/release.sh` one-command cut: sync base, bump, CHANGELOG promote, rebuild Commander, commit, push, open PR, enqueue | local | scripts/release.sh:1-33, prepare :298-358, Commander rebuild :334-337, train receipt check :470-482 | — | — | ADD one release command | DONE (cas-f95b, 20561bfeb); 509 lines | KEEP |
| C-35 | Train full-set receipt precondition (`train-ci.py check`) | local + hosted | release.sh:470-482; scripts/train-ci.py:1-27 (604 lines); .github/actions/full-gate-job/action.yml | full suite at train tip | C-16 in queue; every train push reruns the full set (see D) | — | NEW (ea5058099, 3981a8ba4, Nov 6) | MERGE: queue is the proof |
| C-36 | Auto-cut of green trains (daemon) | local daemon | cas-cli/src/mcp/tools/core/workflow/release_auto_cut.rs:1-12 (272 lines) | — | — | — | NEW (aa42b003f, Nov 8) | KEEP if it removes hand-offs |
| C-37 | Full release gate (43 rows, scheduler) | local; called only by scripts/assembly-proof.py:471,932 | scripts/release-gate.sh:19-25, rows :2333-2427, `merge_queue_rows` :244 | duplicates C-09…C-28 | 37 of 41 rows (a677) | DELETE | PARTIAL: off the release path ("The local train and gate scripts are not called", release.sh:26); file grew 2,389→2,514 | DELETE (keep the `--fast-rows` catalog elsewhere) |
| C-38 | Fast-row catalog (20 rows) | — | scripts/release-integration-gates.py:5-12 (24 lines); release-gate.sh:191-194 | see C-02 | — | DELETE / MOVE | PARTIAL (114→24 lines) | MOVE to the lane lint job |
| C-39 | Policy pins | — | scripts/ci_tiers/policy.json (3 pins, 5 lines), prose-pins.json (15), executable-contracts.sh (689), test-policy.py (1,028) | — | — | DELETE text pins | PARTIAL: parity enforcer and 676 pins deleted (a100b1d4a, Oct 14); 3 pins + 1,717 lines of contract tests remain | DELETE most |
| C-40 | Web-input classifier (decides C-12/C-13) | hosted | scripts/classify-web-inputs.py:1-25 (100 lines); action.yml:130-150 | avoids 19-min web step on unrelated diffs | — | — | STILL PRESENT | KEEP |
| C-41 | Journey selection for QA (`journeys-for-diff.py`, 850 lines) | local close gate / QA | close_ops.rs:32832; qa_journeys.rs:63,181-184; qa_pass.rs:163 | — | CI runs all journeys in 6 shards (no diff selection) | — | STILL PRESENT | KEEP for QA; CI unchanged |

</details>

<details open data-collapse>
<summary>E. cas-a677 scorecard: what its October 15 delete list removed</summary>

Table: cas-a677 §10.5 deletions

| a677 item | a677 size | Now | Status | Evidence |
|---|---:|---:|---|---|
| release-gate.sh + scheduler | 2,389 | 2,514 | PARTIAL | Off the release path (release.sh:26). Still called by assembly-proof.py:471,932 and in `--fast-rows` mode by CI and pre-push. 43 rows |
| release-train.sh, release-train.d/, resume | 3,091 | 0 | DONE | 6253627f6 (Oct 31, cas-b1af1) deleted release-train.sh, release-train-resume.py, release-train.d/ (15 files) and 6 test files. The *concept* returned as `epic/release-train-*` branches + train-ci.py (604) + 2,204 Rust lines |
| assembly-proof.py, assembly-memory.py | 1,634 | 1,642 | NOT DONE | Still the `ASSEMBLY_PROOF` fallback named in AGENTS.md; builtins.rs references it |
| host_memory.py, worker-memory.py + npm wrappers | 976 | 979 | NOT DONE (wrappers PARTIAL) | hub-web no longer routes through worker-memory (classify-web-inputs.py:12-15, cas-a1b0); files remain |
| release_scratch.py, release_env.py | 1,381 | 1,543 | NOT DONE (grew) | callers: release-gate.sh, assembly-memory.py, factory_target_cache/scratch.rs, proof_target.py |
| release-integrate.py, release-integration-gates.py, check-lane-fast-rows.py, fast-rows-pre-push.sh | 878 | 143 | PARTIAL | 2 deleted (6253627f6); gates 114→24; pre-push 57→119 |
| release-learning, -budget-breach, -metrics, -interventions, -latency-receipt, -completion | 1,140 | 1,344 | NOT DONE (grew) | latency receipt 147→350; release-completion.py has no production caller |
| check-ci-gate-row-parity.sh; policy.json pins | 62 + 676 pins | 0 + 3 pins | DONE | a100b1d4a (Oct 14 10:51 PM ET) |
| Daemon: rolling_integration, merge sweep, epic_gate, factory_sweep_tasks, worktree/sweep, background journey eval, e369 re-enqueue | ~9,800 | −10,658 deleted | DONE except worktree/sweep | 9fa37c766 (Oct 15, cas-fd0a). worktree/sweep.rs + opportunistic remain (1,874 lines with tests) |
| Close gates: park fast-rows receipt, journey-receipt gate, lane compile receipt | in close_ops.rs | — | PARTIAL | fast-rows receipt gone (a79bccdb6, cas-51ff); check-lane-compile.py gone (6253627f6); the QA evidence gate still asks for journey evidence on user-facing paths (qa_evidence_gate.rs:95). close_ops.rs grew 39,390→46,913 lines |
| Self-hosted queue routing, self-hosted-fast-validation.yml, runner prune service, self-hosted watchdogs | 127 + units | — | PARTIAL | workflow deleted (20561bfeb); queue var disabled; routing code (ci.yml:255-282), prune timer and runner units remain; lanes went self-hosted |
| Release PR + second queue cycle; release/** and main-push prebuilds; local publisher | — | — | DONE / PARTIAL | The version rides in the train PR (release.sh:10-21). There is one queue run per release (9 merge_group runs for 9 releases since Nov 7). The local publisher is gone. The main-push prebuild became a dependency seed (PARTIAL) |
| `cas/full-gate`, `cas/full-journeys` statuses + skip logic | — | — | NOT DONE (reworked) | ci.yml:2186-2193; check-ci-merge-queue-validation.sh:5; journey-receipt.py:224; train-ci.py:387 |
| Receipts stage, delivery-completion SHA check, stage-budget P0s, recurrence escalation, failure-log gate | — | — | PARTIAL | receipts.sh gone with release-train.d; `failure-log` is still fast row 1 (release-integration-gates.py:6); release-stage-budgets.json (15) present; release-completion.py orphaned |
| `full_gate_required`, `integration-no-build-tip` | — | 1 string | MOSTLY DONE | only release-gate.sh:582 prints `full_gate_required` |
| Script tests for the above (~60 files, ~20,000 lines) | 27,228 lines | 28,672 | NOT DONE (grew) | 86→97 test files |
| Rust sweep/epic-gate tests (~150) | — | gone with 9fa37c766 | DONE | epic_gate_tests.rs, merge_queue_recovery_tests.rs deleted |
| Dead code class (c), 79 items | — | partial | PARTIAL | removed: notifications pipeline (0bc495259), ambient recall (b19f3892b), search metrics (0ecc52c68), Jev (91427a334, Oct 14). Not re-measured |
| Orphan script tests (9,192 lines) | — | — | NOT MEASURED | — |

Table: cas-a677 §10.7 additions

| # | Addition | Status | Evidence |
|---|---|---|---|
| 1 | One toolchain + tool manifest | DONE for Rust and markdownlint; node not pinned | rust-toolchain.toml channel 1.99.0 (53ef1a113, Oct 14); package.json `markdownlint-cli2: 0.18.1` (6310aebf6); no `engines`/node pin in package.json |
| 2 | Independent queue jobs with `!cancelled()` | PARTIAL | web, journeys, tiers, bridge, docs and suite are separate jobs; preflight's Rust steps stay serial (ci.yml:900-968) |
| 3 | Journeys in hosted shards | DONE (6, not 3) | cf2211ba2; ci.yml:1238-1281 |
| 4 | Release workflow triggered by the version change on main | DONE | release.yml:27-31 (20561bfeb) |
| 5 | Install on publish | DONE | install-on-publish.sh; timer; 8 of the last 10 releases record "installed" |
| 6 | One timer (PR opened → installed) replacing latency receipt, stage budgets, REQUEST/CUT metrics | PARTIAL | timer in the notes (release-publish.sh:10-14, install-on-publish.sh:19-22); release-latency-receipt.sh (350), release-metrics.py (245), release-stage-budgets.json and `cas ledger estimate` (release.sh:490-495) remain |
| 7 | `-D dead_code` on the PR build | NOT DONE | no `dead_code` in Cargo.toml, .cargo/config.toml or ci.yml |

</details>

<details open data-collapse>
<summary>E. Generated and shared docs, and their churn since October 27</summary>

Table: Generated docs, who writes them and what checks them

| Artifact | Generator file:line | Who writes it today | Freshness checks file:line | Conflict evidence (since Oct 27) | Proposal: generate once at release cut |
|---|---|---|---|---|---|
| `.claude/CODEMAP.md` | none deterministic. The codemap skill (builtins.rs:495-500) has an agent rewrite it. A PostToolUse hook records structural changes to `.cas/codemap-pending.json` (hooks/handlers/handlers_middle/post_tool.rs:157 → handlers_events/codemap.rs:36-90) | every worker in every delivery (G5 refusal at park) | park G5 `CODEMAP DRIFT`, enforce (close_ops/docs_upkeep.rs:56,136-141; called close_ops.rs:9138; `.cas/config.toml` `[upkeep] codemap = "enforce"`); SessionStart banner (handlers_session.rs:256,387); stop reminder (codemap.rs:557); gate preflight (close_ops/gate_preflight.rs:122); QA impact reads area labels (qa_impact_io.rs:158,285) | 261 non-merge + 251 merge commits touch it; in 7 of 43 `# Conflicts:` lists; mentioned in 10 conflict commits | Generate from the tree at release prep (release.sh prepare). Drop G5 at park and the per-commit hook; keep the SessionStart banner advisory |
| `docs/PRODUCT_OVERVIEW.md` | none deterministic. The project-overview skill has an agent write it; a hook records watch-dir changes (post_tool.rs:169-177 → project_overview.rs:388) | every worker whose delivery touches `domain_paths`/`watch_dirs` or a live journey | park G6 `PRODUCT_OVERVIEW DRIFT`, enforce (docs_upkeep.rs:57-70,143-149); SessionStart freshness (handlers_session.rs:410 → project_overview.rs:589-599); skill text (project-overview/SKILL.md:153) | 301 non-merge + 304 merge commits; in 8 of 43 conflict lists; 11 conflict commits mention it | Same: one regeneration at the cut, or a release-notes-time review; no park refusal |
| `docs/qa/journeys.yaml` (QA registry: journeys, owners, bug_families) | hand/agent-edited; a custom merge driver unions bug_families (scripts/cas-merge-drivers.py:20-23; .gitattributes `merge=cas-journeys`; installed by worktree/git.rs:170-209) | workers (G1 broken registry, G2 unowned user-facing file, G3 bug fix without a case) | park G1–G3, `[qa.registry] enforce = "enforce"` (close_ops/registry_upkeep.rs:11-40,195-218,729); default path qa_registry.rs:9; `journey-catalog` fast row checks journeys.md vs test.step titles (release-gate.sh:1775,2410) | 277 non-merge + 220 merge commits; **the most conflicted file**: 10 of 43 conflict lists, 12 conflict commits | Keep it hand-owned (it is source, not output); move G2 ownership to a release-time report; make G1/G3 advisory. bug_families could live per-file (one file per family, like changelog.d) to end conflicts |
| `CHANGELOG.md` + `changelog.d/<task>.md` | `scripts/release_changelog.py promote` folds fragments into a dated section at the cut (release.sh:341-347) | workers write fragments; release.sh writes CHANGELOG | park/gate preflight demands a fragment or "no changelog" note (close_ops/gate_preflight.rs:88-97; receipt_scope.rs:257,278-287); promoted-fragment reconciliation (close_ops.rs:14750-14775); `changelog-and-versions` fast row (release-gate.sh:1610,2387); `cas-changelog` merge driver (.gitattributes; cas-merge-drivers.py:12-18) | CHANGELOG.md 482 non-merge + 365 merge commits; changelog.d 248 + 231; CHANGELOG in 4 conflict commits, changelog.d in 4. Fix commit bdb44b281 (Nov 3): "take per-delivery changelog fragments so queued PRs stop conflicting" | Already "generate once at the cut" (fragments → promote). Keep. Delete the park fragment demand if release notes are written from PR titles |
| `hub-web/dist/**` | `npm run build`; regenerate after every merge (scripts/regenerate-generated-artifacts.sh:21-60, called by worktree/git.rs:1407-1416); rebuilt at the cut (release.sh:334-337) | merge machinery, release.sh, workers | committed-dist drift in queue (ci.yml:1188-1200, policy from classify-web-inputs.py `--dist-policy`); `hub-web-dist-drift` gate row (release-gate.sh:244) | 234 non-merge + 36 merge commits; 1 conflict mention; `merge=cas-generated` keeps ours | Build only at the cut (release.sh already does) and in release.yml; stop committing per-merge rebuilds |
| `cas-cli/src/builtins/reference-history.json` (+ Codex verifier projections) | scripts/gen-builtin-reference-history.sh via regenerate-generated-artifacts.sh:62-103 | merge machinery | close_ops.rs:14926-14999 (regenerated-history reconciliation); release-gate.sh:128-129,259 | 145 non-merge + 6 merge commits; 0 conflict mentions | Generate once at the cut |
| AGENTS.md / CLAUDE.md | `cas sync agents-md` | agents | Docs Lint `--check` + 10,000-char cap (ci.yml:219-248) | — | AGENTS.md is canonical (CLAUDE.md imports it), so the check exits early (ci.yml:235-238) |

Table: Commits touching each generated file since October 27

| Path | `git log origin/main --since="$SINCE" --oneline -- P \| wc -l` | `--no-merges` | `--first-parent` |
|---|---:|---:|---:|
| docs/qa/journeys.yaml | 497 | 277 | 39 |
| .claude/CODEMAP.md | 512 | 261 | 56 |
| docs/PRODUCT_OVERVIEW.md | 605 | 301 | 59 |
| CHANGELOG.md | 847 | 482 | 46 |
| changelog.d/ | 479 | 248 | 61 |
| hub-web/dist/ | 270 | 234 | 43 |
| reference-history.json | 151 | 145 | 25 |
| all commits | 6,453 | 4,790 | — |

</details>

<details open data-collapse>
<summary>E. Release timing, last 10 releases (Eastern)</summary>

Table: Release PR opened → installed

| Release | PR | PR opened | Queue run | Merged | Published | Installed | open→merged (min) | queue run (min) | merged→published (min) | release.yml run (min) | open→published | open→installed |
|---|---|---|---|---|---|---|---:|---:|---:|---:|---:|---:|
| v4.12.0 | #1744 | Nov 10 7:08 AM | 7:09–7:27 AM | 7:27 AM | 7:42 AM | 7:44 AM | 19.4 | 17.6 | 14.5 | 15.5 | 33.8 | 35.4 |
| v4.11.0 | #1737 | Nov 9 10:45 PM | 10:46–11:03 PM | 11:03 PM | 11:08 PM | 11:10 PM | 17.8 | 16.2 | 5.2 | 6.3 | 23.0 | 24.7 |
| v4.10.0 | #1723 | Nov 9 7:30 PM | 7:31–7:50 PM | 7:50 PM | 8:00 PM | 8:02 PM | 20.2 | 18.4 | 10.3 | 11.4 | 30.5 | 32.3 |
| v4.9.0 | #1708 | Nov 9 11:21 AM | 11:23–11:40 AM | 11:40 AM | 11:57 AM | 11:59 AM | 18.4 | 16.7 | 16.9 | 18.2 | 35.4 | 37.1 |
| v4.8.0 | #1682 | Nov 8 11:31 PM | 11:32–11:53 PM | 11:53 PM | 11:57 PM | 11:58 PM | 22.6 | 21.2 | 3.4 | 4.5 | 25.9 | 27.3 |
| v4.7.0 | #1653 | Nov 7 11:54 PM | 12:01–12:19 AM | Nov 8 12:19 AM | 12:25 AM | 12:26 AM | 24.9 | 17.8 | 5.7 | 6.8 | 30.6 | 31.9 |
| v4.6.0 | #1651 | Nov 7 6:53 PM | 7:00–7:17 PM | 7:17 PM | 7:22 PM | 7:24 PM | 23.4 | 16.9 | 5.6 | 6.7 | 29.0 | 30.4 |
| v4.5.0 | #1648 | Nov 7 3:15 PM | 3:17–3:33 PM | 3:33 PM | 4:01 PM | 4:03 PM | 17.5 | 15.9 | 28.0 | 29.1 | 45.5 | 47.5 |
| v4.4.0 | #1638 | Nov 7 12:25 PM | 12:31–12:46 PM | 12:46 PM | 12:55 PM | pending | 21.5 | 15.8 | 8.6 | 9.7 | 30.1 | — |
| v4.3.0 | #1621 | Nov 6 4:58 PM | 3 queue runs, 5:29–6:38 PM (prebuild) | 6:26 PM | 6:29 PM | pending | 87.7 | — | 3.2 | 4.3 | 91.0 | — |

</details>

<details open data-collapse>
<summary>F. All 21 incidents, classified</summary>

Table: Incident ledger (times Eastern)

| ID | Incident | When (ET) | What happened | Mechanism | Cost | Class | Move |
|---|---|---|---|---|---|---|---|
| I-01 | cas-7584, GH #1752 | Filed 9:07 AM Nov 10; closed 9:30 AM | A no-commit batch (`git clone --shared --no-checkout`, `sqlite3 -readonly .backup`, `git -C clone checkout -b replay SHA`) was denied as WORKER COMMIT GUARD. It blocked the cas-9c84f replay fixture. | PreToolUse shell-text classifier. `pre_tool.rs:2803` treats `checkout -b` operands as a history write; `:2914` resolves -C; `:3099` refuses; deny emitted at `pre_tool.rs:12589` | 1 GH issue, 1 bug task, 1 cross-machine BUILD_ROUTE (soundwave routed it to prowl at 9:08 AM), about 23 worker-minutes. The 4.13 replay was blocked until the merge (59a6f5f3e) | FALSE POSITIVE | (3) |
| I-02 | cas-84ea, GH #1615 | Task created 8:49 PM Nov 8; still in progress 9:44 AM Nov 10 | `W=<worktree>; cd "$W"; git commit` from apps/backend was denied with a false "HEAD detached" while the branch was a named factory branch. | Commit guard cwd and branch resolution: `git_write_targets` skips assignment statements, `resolve_worker_cd_path` expands only CAS_CLONE_PATH, and the branch lookup collapses Git errors to None (task note 9:35 AM Nov 10) | Carried across 3 trains without a fix ("Release migration: not included" twice: cas-db63 to cas-4088 to cas-16ff to cas-2e0e). Preempted at 9:31 AM Nov 10 by the I-12 conflict repair | FALSE POSITIVE | (3) |
| I-03 | cas-ce2f, GH #1754 | Filed 9:16 AM Nov 10; open | A read-only `ssh peer python3 -` script (sqlite `mode=ro` plus `glob.glob('~/*/*/gabber')`) was denied as WORKER-DENIED OPERATOR CREDENTIALS (Neon/Vercel). | Credential path guard. `operator_credentials.rs:784` sends quoted script paths to the generic glob matcher; `glob_path_may_reach_root` lets the `*` match a hidden `.config`, reaching `~/.config/neonctl` | Blocked the cas-e75c diagnosis. 1 GH issue, 1 task. Compile proof hit capacity-admission refusals at 9:27, 9:34 and 9:44 AM ET and was still not run at 9:45 AM | FALSE POSITIVE | (3) |
| I-04 | cas-7e73, GH #1753 | Filed 9:10 AM Nov 10; triaged 9:10 AM | `cas config set project.canonical_id` answered "Unknown config key". The config guard denies direct config.toml writes and points at `config set`, so the worker found no route. The admitted route was `cas cloud project set`. | Config-key registry versus Cloud project pin (two owners for one key) plus the config.toml write guard | 1 GH issue, 1 task, supervisor triage (route verified on the replay fixture at 9:11 AM). Remaining fix: the refusal should name the owning command | MISSING FACT (key ownership split; partly a NON-BUG, since a route existed) | (2) |
| I-05 | cas-c1fb | 1:45 PM and 2:30 PM Nov 6 | progress_stalled fired twice for 13 prowl tasks that were merged or parked and waiting on soundwave's train and batch_receipt. Prowl had 0 workers. It threatened an operator page. | Watchdog `progress_watchdog.rs:35` kept every assigned open task, with no containment or peer-ownership check | 2 false blocker notices, 1 page threatened, 1 task. Fixed by containment ancestry plus binding-split exemption | MISSING FACT (delivery containment and peer ownership not visible to the watchdog) | (2) |
| I-06 | cas-1f65 | 4:53 AM and 5:39 AM Nov 9 | Train cas-db63 was frozen at 2:58 AM (operator-only lift). progress_stalled listed its parked children (cas-0f47, cas-1d99) as owed by the supervisor and threatened a page. | Watchdog `progress_watchdog.rs:45-84` never consulted the train-freeze label | 2 false notices, 1 overnight page threatened, 1 task. The task's own close at 8:06 PM ET needed a reviewed-drop override (CODEMAP plus fragment, see I-13) | MISSING FACT | (2) |
| I-07 | cas-098dd | 9:31 AM Nov 10 (gabber) | The only open item was epic cas-6aef: 18/18 children closed, verification approved, delivery on main. Its close was refused by the dead-target-branch guard (GH #1747). progress_stalled warned that the operator would be paged in 45 minutes. | Watchdog `progress_watchdog.rs` (around :69): any non-awaiting_merge task counts as actionable, and refusal codes are not stored where the watchdog reads | The supervisor had to set 3 finished tasks to blocked (cas-6aef, cas-68ad, cas-8ffd) to stop the page. 1 task. This is the third special case in the same family (I-05, I-06) | MISSING FACT (close-refusal state not stored where the watchdog reads it) | (2) |
| I-08 | cas-c74f8 | 9:17 AM Nov 10 | spawn request 3638, then shutdown request 3639 during boot. The director confirmed the shutdown, then notification 101899 said "worker harness failed during boot, reassign/respawn". | `reconcile_spawn_verifications` failure branch (`queue_and_events.rs`, `DeathRelay::Alarm`). The Shutdown handler left the pre-registration watch in place, so the 60-second registration timeout fired | 1 misleading relay, 1 task. Already covered by cas-7076 (4058c34ac) | MOVING-STATE RACE (shutdown intent vs a spawn watch in a separate store) | (2) |
| I-09 | cas-23a6 | 9:28 AM Nov 10 | Build-overflow routing queued peer_message_outbox row 1200 with the whole serialized task (68,762 characters). Cloud returned 400 (cap 64,000) and the watchdog abandoned the row, so the peer never got the staffing ask. | `factory_build_routing.rs:72-76` embeds `serde_json::to_string_pretty(task)`. The size comes from note bloat (see the cross-cutting notes) | 1 lost staffing ask (cas-b200), 1 task, a silent failure | MISSING FACT, closest fit (the route copies a mutable task instead of citing an id in the shared store) | (2) |
| I-10 | cas-f5c2 | Nov 10 (gabber); task 9:11 AM. Incident time UNVERIFIED | Backfill or reviewed-drop closes of released QA tasks (cas-27fc, cas-f580) hit gates that were added after their approval: "NO-CODE INTENT VIOLATED" and "QA LEDGER REJECTED before verification dispatch" (legacy Oct 29 ledger format). | Close gates evaluated against current rules, not the rules in force at approval | The gabber supervisor cancelled 2 finished tasks instead of closing them. 1 task | MOVING-STATE RACE (gate rules moved after approval) | (1) and (4) |
| I-11 | cas-0e49, GH #1730 (peer wake cap) | Filed 8:49 PM Nov 9 (gabber release); prowl capped on Nov 9 | A peer "window open, go" message was held 613 seconds by the 6/hour wake cap, and the sender was not told. | Peer wake scheduler: `:297-315` queues only a receiver notice, `:332-338` reparks silently, `waking_kind :142-148` ignores a linked Status reply | About 10 minutes lost on a time-critical release gate. 1 GH issue, 1 task | MISSING FACT (the hold existed only on the receiver; nothing derived a notice for the sender) | (2) |
| I-12 | cas-0e49 (conflict) and cas-84ea preemption | Parked 9:23 AM, returned 9:30 AM, repaired 9:38 AM, re-parked 9:44 AM Nov 10 | The park hit a genuine conflict on `docs/PRODUCT_OVERVIEW.md` and `docs/qa/journeys.yaml` (same-journey owners and invariants, which the driver keeps conflicting by design). | merge_conflicted return; cas-journeys merge driver; the PRODUCT_OVERVIEW upkeep gate | About 21 minutes, 1 worker round trip, 1 extra capped test run (art-b485ec92). The worker was pulled off cas-84ea (I-02) | GENERATED-DOC CONFLICT | (4) |
| I-13 | Tonight (a): content-drop refusals. cas-2fe8 (GH #1726), cas-9ada, cas-0460, cas-1f65, cas-45e7 and 29 more | 7:41 PM Nov 9 to 7:50 AM Nov 10 (v4.10.0 published 8:00 PM ET) | After each release, closes of shipped deliveries were refused DELIVERY CONTENT DROPPED for paths the release or later merges changed by design: the release compiles and deletes `changelog.d/<task>.md`, and CODEMAP, journeys.yaml and PRODUCT_OVERVIEW are regenerated or unioned. | `close_ops.rs:15732 anchored_delivery_content_gate_unbounded` (refusal text `:15886`). Cause 1: `close_ops.rs:14757 without_promoted_changelog_fragments` walked deletions without `--full-history`. Cause 2: no generated-CODEMAP exemption, added by cas-2fe8 at `:25860` | 36 supervisor reviewed-drop overrides, each hand-written, on 34 unique tasks (cas-ad2c and cas-4c6f twice). 28 of 36 named only changelog fragments, CODEMAP, journeys.yaml or PRODUCT_OVERVIEW; 8 also named source or snapshot files. Dropped-path counts: fragment 26, journeys.yaml 9, CODEMAP 8, source 9, PRODUCT_OVERVIEW 3. 3 cleanup tasks (cas-2fe8, cas-9ada for 46 deliveries, cas-0460 for 14 closes). cas-2fe8 was fixed at 8:34 PM ET and shipped in v4.11.0, yet 21 more overrides came 9:59-10:38 PM and 7 more 6:57-7:50 AM | GENERATED-DOC CONFLICT, plus FALSE POSITIVE for the fragment walk | (1) and (4) |
| I-14 | Tonight (b): journeys.yaml merge conflicts. GH #1733, cas-ded57; merge_conflicted returns on cas-16c0a, cas-1a73 (x2), cas-bf2f, cas-0e49 | 9:06 PM, 9:14 PM, 9:25 PM and 10:12 PM Nov 9; 9:30 AM Nov 10 | Each delivery inserted its bug_family at the top of `bug_families:` and rewrote `updated:`. Parallel additive deliveries conflicted, "3 of 4 tonight", one twice because the train moved during its ~4-minute lane CI. | worktree_merge CONTENT CONFLICT. Fix: cas-journeys merge driver (`scripts/cas-merge-drivers.py`, `.gitattributes:27`), cas-ded57 4da5cbe95 at 9:42 PM ET, shipped in v4.11.0. `.gitattributes` itself notes that GitHub's mergeability check and merge queue ignore custom drivers | Git replay since 6:00 PM ET Nov 9: 42 two-parent merges touched journeys.yaml. 16 conflict under plain text merge; 4 still conflict with the driver (fc137e7e7 10:29 PM, d5667bb89 8:53 AM, f5ca6f08f 9:30 AM, 5dcc8f8f6 9:37 AM). 5 returns to workers, each with a fresh lane CI. 9 of the I-13 overrides also name journeys.yaml | GENERATED-DOC CONFLICT | (4) |
| I-15 | Tonight (c): installer hang. cas-a00be (P0), GH #1750 | Install began 11:08 PM Nov 9 (finished in 164 s); the notice child hung about 11:11 PM until killed at 8:42 AM Nov 10 (9 h 30 min) | The LaunchAgent `dev.cassy.install-on-publish` child `cas release installed --receipt ... pending.json` (pid 23142) sat in poll() and held the installer lock and cas.db. v4.12.0 (published 7:42 AM ET) was not installed at 8:41 AM; `cas --version` reported 4.11.0. | Root cause is inconclusive (parent-only stack sample). Most likely an unbounded `git ls-remote` `.output()` in `release_automation.rs` `published_tag_sha :721-724`, or other unbounded Git calls (`:83-88`, `:460-462`). Daemon wait ruled out (nonblocking datagram, `notify_daemon :103-113`). Fix: `bounded_process` (30 s Git cap), 45 s notice deadline, per-version pending receipts, install-before-retry ordering. Detection: no watchdog fired. The supervisor saw it by hand about 1 hour after v4.12.0 published | 9.5 hours of a blocked installer; v4.12.0 about 1 hour late on prowl. v4.12.0 carried the expired-mirror alert fix, which prolonged I-16. 1 P0 task, 1 GH issue | MISSING FACT ("published SHA X, installed SHA Y" was asserted nowhere) | (1) |
| I-16 | Tonight (d), the flood actually found: transcript-mirror blocker flood. cas-c568, cas-0afdd; GH #1738, #1740, #1742 | 11:10 PM Nov 9 (2 minutes after the v4.11.0 install) to 8:42 AM Nov 10 | delivery-watchdog raised a `cas-blocker` wake for each day-old transcript mirror from the ended session subtle-gazelle-36 as it aged to superseded_stale. Prowl had about 470 such unacked rows. | delivery-watchdog "Operator messages not delivered before transcript expiry". It re-derived blockers from outbox rows and never checked which factory session owned them | prompt_queue (read-only count): 98 blocker prompts to lively-fox-47 (11:10 PM to 8:42 AM). The supervisor answered "Ignored." 84 times (12:44 to 7:05 AM) and "Another stale reply... Ignored." 6 times (11:15 to 11:42 PM). The fixes closed at 6:57 AM ET but were not installed until v4.12.0 (blocked by I-15) | MISSING FACT (session ownership not part of the notice's facts) | (2), with (1) for install |
| I-17 | Tonight (d), verification-dispatch flood as described in the brief | Window 5:00 PM Nov 9 to 10:00 AM Nov 10 | NOT FOUND on prowl. verification_dispatches requested in the window: 35 (30 resolved, 4 invalidated, 1 pending). Most for one task: 3 (cas-aa18, 2 invalidated). supervisor_queue: 1 verification_dispatch_expired, 4 verification_approved. The literal string "VERIFICATION REQUIRED" appears in no task notes or close reasons for tasks updated in the window. | n/a | n/a | UNVERIFIED. It may be on soundwave's store, which was not read (no SSH used) | n/a |
| I-18 | Guard false positive hit while gathering this evidence | About 9:50 AM Nov 10 | A read-only bash loop counting merges (`n=$((n+1))` with `git merge-tree`) was refused: "FACTORY WORKSPACE CONTRACT: cannot resolve shell filesystem effects (an unresolved arithmetic operand)". The identical logic in Python ran. A second refusal (writing `/tmp/close_ops_main.rs`) was policy working as designed. | Factory workspace shell-effects parser (same family as closed GH #1711, the `"$?" -eq 1` arithmetic case) | 2 retries. No issue filed (read-only agent) | FALSE POSITIVE (first refusal); TRUE CATCH (second, /tmp write) | (3) |
| I-19 | cas-1811 (gabber cas-3983) | Task 8:24 AM Nov 10; closed 9:03 AM | Reported as close output saying "closed" while the task was parked. The original row reads "NOT CLOSED - COMMITTED: parked awaiting_merge", which was honest. | Close disposition receipts: `close_ops.rs:6510` (parked), `:12049` (closed); reported by `server_handler.rs:565/:603` | 1 task, about 11 worker-minutes, 2 artifacts, 1 supervisor negative-result close. The word "COMMITTED" ahead of "parked" invited the misread | NON-BUG | none ((2) would make the final status the only answer) |
| I-20 | cas-99c7 (gabber cas-27fc, cas-f580) | Task 8:24 AM Nov 10; closed 9:10 AM | Reported as a verdict rejected because a ledger changed after dispatch. The ledgers were actually deleted on purpose in gabber efce1e4e2 ("keep QA ledgers as task artifacts"); neither blob exists at staging 8bd452f0. | DELIVERY CONTENT DROPPED, `close_ops.rs:15886` (`anchored_delivery_content_gate_unbounded`), not `repository_proof.rs:730` | 1 task, 2 measurements (art-8b0c7fbc, art-8b1da187), 1 supervisor negative-result close, then I-10 | NON-BUG; the gate made a TRUE CATCH of a real deletion | (1) (release-made supersession proven once) |
| I-21 | Prowl "3-hour stall" (from the supervisor's operator message 5:01 PM ET Nov 9; not task-verified) | About 2:00 to 5:00 PM Nov 9 | After a 4.9.0 restart, workers for epic-verification timeout and Slack routing came back, but their tasks stayed blocked: "Cassy's restart brief doesn't resume a blocked task." | Restart brief versus blocked_hold status | About 3 hours of idle workers; the supervisor issued explicit go orders | MISSING FACT. UNVERIFIED beyond the message text | (2) |

</details>

<details open data-collapse>
<summary>F. Defect baseline per release</summary>

Table: Cassy defects filed per release window

| Release | Window length | GH issues | GH agent-filed | GH "flaky" titles | GH issues/hour | Cassy bug tasks | of which from-finding |
|---|---|---|---|---|---|---|---|
| v4.5.0 | 3.1 h | 10 | 0 | 1 | 3.23 | 22 | 16 |
| v4.6.0 | 3.4 h | 1 | 0 | 1 | 0.30 | 2 | 1 |
| v4.7.0 | 5.0 h | 2 | 0 | 2 | 0.40 | 12 | 12 |
| v4.8.0 | 23.5 h | 28 | 8 | 2 | 1.19 | 83 | 26 |
| v4.9.0 | 12.0 h | 27 | 15 | 1 | 2.25 | 36 | 27 |
| v4.10.0 | 8.1 h | 13 | 6 | 2 | 1.61 | 16 | 11 |
| v4.11.0 | 3.1 h | 11 | 2 | 1 | 3.52 | 8 | 5 |
| v4.12.0 | 8.6 h | 9 | 4 | 1 | 1.05 | 7 | 4 |

</details>

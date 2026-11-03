# Fast lane for small frontend-only hotfixes: design brief

Task cas-8156, epic cas-ae20. This is a design only; it adds no code.
Status: proposed, Nov 3 2026. It changes the merge and release flow, so Daniel
must approve it before any of it is staffed.

## Why

During a client demo recording on Nov 3 (about 3:50 PM ET), Daniel asked
pulse-card for a CSS grid fix plus one summary line on the admin creator page.
The honest ETA came back as about 30 minutes. The fix itself (PR #613, task
cas-cc61) shipped to production 15 minutes 37 seconds after the task was
created. It only managed that because the supervisor merged at 4:05 PM, before
hermetic e2e, the fresh build and the PostgreSQL contracts had finished. No
receipt was owed for those checks afterwards. They happened to pass at
4:11 PM.

So there are two problems:

1. The gates as written cost about 20 minutes, and the ETA reflects that.
2. The only way to go faster today is an unrecorded judgment call that skips
   them.

The lane below makes the fast path mechanical and covered by a receipt.

## Measured before

Sources:

- pulse-card GitHub runs and deployments.
- The pulse-card task store (`.cas/cas.db`, events for cas-cc61).
- The worker transcript (quick-condor-88, session db138ada).

### Today's hotfix, PR #613 (5 files, 191 changed lines)

| Step | ET | Duration | Source |
| --- | --- | --- | --- |
| Task created to worker started | 3:53:44 to 3:55:09 PM | 1:25 | cas.db events |
| Worker reads and edits | 3:55:09 to 3:57:14 PM | 2:05 | transcript |
| Local prettier, `typecheck:baseline`, scoped vitest (398 tests) | 3:57:23 to 3:58:08 PM | 0:45 | transcript |
| Commit and push d2e83e82 | 3:58:13 to 3:58:15 PM | 0:02 | transcript |
| Before-screenshots of production, taken before the PR | 3:58:21 to 3:59:50 PM | 1:29 | transcript |
| Push to PR open: CI starts only when the PR opens | 3:58:15 to 4:00:09 PM | 1:54 | check rollup |
| CI: Frontend unit tests (vitest + vue-tsc baseline) | 4:00:09 to 4:04:54 PM | 4:45 | run 41717717249 |
| CI: Frontend fresh build | 4:00:09 to 4:05:52 PM | 5:43 | run 41717717249 |
| CI: backend boot, typecheck, unit, fresh DB | 4:00:09 to 4:03:05 PM | 1:32 to 2:56 | run 41717717198 |
| CI: PostgreSQL contracts | 4:01:55 to 4:08:10 PM | 6:15 | run 41717717198 |
| CI: hermetic e2e (build 3:53, specs 5:57) | 4:00:09 to 4:11:04 PM | 10:55 | run 41717717001 |
| Merge (merge commit df10bda9), with e2e still running | 4:05:32 PM | 5:23 after CI start | PR API |
| Vercel production deploy | 4:05:36 to 4:09:21 PM | 3:45 | merge-commit status |
| Task close | not closed at 4:13 PM | open | cas.db |

There is no Vercel preview today. Every PR head goes from pending to
"Canceled by Ignored Build Step" within 6 to 57 seconds. The rule lives in the
Vercel dashboard; it is not in `apps/frontend/vercel.json`.

### Ten earlier small frontend-only PRs

The sample is PRs 611, 610, 608, 607, 606, 604, 597, 595, 578 and 534. Every
one of them merged only after all checks were green.

| Step | Median | Max |
| --- | --- | --- |
| Push to all checks green | 11:09 | 22:03 (534) |
| All green to merge | 0:36 | 1:16 |
| PR created to merge | 12:00 | 25:49 (606) |
| Merge to Vercel production success | 4:06 | 5:26 |
| Hermetic e2e job (the long pole) | 10:43 | 11:52 |
| Frontend fresh build job | 6:17 | 7:22 |
| Frontend unit tests job | 4:50 | 5:08 |
| PostgreSQL contracts job | 5:30 | 6:17 |

Following the gates as written, request to production is about 20:08:

- 1:25 to start the worker
- 2:05 to edit
- 0:45 for local checks
- 11:09 from push to all green
- 0:36 from green to merge
- 4:06 for the Vercel deploy

The Cassy close debt comes on top of that.

The repository setup has three facts that matter here:

- pulse-card's plan returns 403 for branch protection and rulesets, so GitHub
  requires no checks.
- Auto-merge and a merge queue are both off.
- No PR workflow has a path filter, so a frontend-only PR runs the whole
  backend suite.

## Current gates on this path

A pulse-card frontend hotfix is a Tier 1 (Standard) delivery
(`review_tier.rs:101` `tier_for_paths`). The default `qa.user_facing_paths`
(`config/settings.rs:691`) match `.vue`, `.css`, `.tsx` and similar files, so
it owes a QA bundle.

Abbreviations: `CO` means `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs`
and `L` means `cas-cli/src/mcp/tools/core/task/lifecycle/`.

| Gate | Handler | Defect class it catches |
| --- | --- | --- |
| Proof claims in the close reason | CO:7897 `proof_claims::validate_claims` | an unsupported "it passed" claim |
| Registry upkeep G1, G2, G3 (bug case) | `L/close_ops/registry_upkeep.rs:11` `at_park` | journey registry drift, unowned user-facing files, a bug fix with no regression case |
| Docs upkeep G5, G6 (CODEMAP, overview) | `L/close_ops/docs_upkeep.rs:9` `at_park` | navigation and product docs drifting from the code |
| QA ledger preflight | `L/qa_evidence_gate.rs:203` | a malformed ledger |
| QA evidence / bundle | `L/qa_evidence_gate.rs:503` `qa_evidence_close_gate_for_paths`; batch debt `src/qa_batch.rs:569`, `:577` | a visual or UX defect a unit test cannot see |
| MERGE REQUIRED, unpushed tip, park attribution | CO:9128, CO:9236 to 9324 | work stranded off the target |
| Changelog and focused proof | `L/close_ops/receipt_scope.rs:114` | missing release notes; an untested Rust crate (neither applies to a downstream frontend) |
| Independent QA round | CO:9442, `L/qa_dispatch.rs:719`, `:1274` | per_task mode only (pulse-card runs batch) |
| Verifier | CO:10333 `request_close_verification_dispatch`; post-merge for parked lanes, `merged_verification.rs:5` | missed acceptance criteria |
| Merge CI admission | `src/mcp/tools/core/workflow/worktree_ops.rs:88` `admit_branch_ci` (called from :5008) | red, pending or missing CI on the merged tip |
| QA merge refusal | `worktree_ops.rs:4302`, `:4524` via `src/qa_pass.rs:689` | merging a user-facing delivery whose QA failed |

Today the batch receipt that settles QA debt can only be recorded by the
supervisor, and it needs a staging SHA (`src/mcp/tools/service/qa_pass_ops.rs:15-31`).
pulse-card has no staging step, which is the cas-f426 friction (about eight
supervisor overrides on Nov 3, per epic cas-ae20).

Fast paths that already exist:

- `depth=light` (CO:8210) skips only the verifier.
- The operator's rapid-response window (`src/rapid_response.rs`) is time-boxed
  and creates back-fill tasks.
- There is no automatic revert anywhere (`backlog_hygiene.rs:409` only detects
  reverts).

## Eligibility, measured mechanically

Cassy evaluates eligibility at park, from the delivered diff against the
target's merge base. It is all-or-nothing. Every condition below must hold,
otherwise the delivery takes the normal path.

1. **Paths.** Every changed path matches the project's `fast_lane.paths`. For
   pulse-card: `apps/frontend/app/src/**`, `apps/frontend/locales/**`,
   `apps/frontend/test/**` and `docs/qa/journeys.yaml`.
2. **Deny list.** No changed path matches `fast_lane.deny`. For pulse-card:
   - `apps/frontend/server/**` (Nuxt API routes)
   - `apps/frontend/middleware/**` and `apps/frontend/app/src/middleware/**`
     (route guards)
   - any path containing `auth`, `billing`, `stripe` or `payment`
   - `package.json`, lockfiles, `nuxt.config.ts`, `vercel.json`, `.env*`,
     `.github/**`
   - everything under `apps/backend/**`
3. **Size.** At most 10 files and 300 changed lines. That admits #613 (5 files,
   191 lines) and 9 of the 10 sampled PRs; it excludes #606 (16 files, 408
   lines).
4. **Tier.** The delivery is Tier 1 by `tier_for_paths`, never Tier 2. Nothing
   that enforces rules can ride this lane.
5. **Kind of work.** The task is `type=bug`, or carries a `finding_origin`, or
   has the `hotfix` label. It is not a child of an epic phase.
6. **No migrations, no new route files, no dependency changes.** The path rules
   above already cover these.

Eligibility is recorded on the task as a decision note listing the measured
values (files, lines, matched globs). The verifier and the operator can then
audit it.

## Gates: kept, skipped or deferred

| Gate | Fast lane | Why |
| --- | --- | --- |
| CI Frontend unit tests (vitest + vue-tsc baseline), 4:50 | **Keep: the only merge gate** | It catches logic and type regressions in the changed code. It is the cheapest independent check. The worker's local run (0:45) stays as feedback, but it is not the gate, because worker-written receipts are not independent. |
| Local prettier and typecheck | Keep (worker loop) | Format and type errors, seconds to run. |
| Backend boot, typecheck, unit, fresh DB, PostgreSQL contracts | **Skip** | They catch backend defects. Eligibility forbids every backend and API path, so they cannot fail because of this diff. This is removed with a pulse-card path filter; no Cassy check is added. |
| Frontend fresh build, 6:17 | **Defer to the Vercel production build** | It catches a broken production build. Vercel builds the merge commit anyway (3:45). A failed Vercel build is never promoted, so production keeps the previous deployment and the receipt below fails. |
| Hermetic e2e, 10:43 | **Defer to post-merge** | It catches cross-page integration regressions. It already runs on push to main (`e2e-hermetic.yml`), so the lane waits for that run instead of a second one on the PR. |
| QA bundle and batch receipt (`qa_evidence_gate.rs:503`, `qa_batch.rs:569`) | **Defer to the post-merge receipt** | It catches visual and UX defects. These are checked on the deployed page, which is where the defect would show. A pre-merge staging receipt adds nothing for a frontend-only diff. |
| Upkeep G1, G2, G5, G6 | **Defer: observe at park, one follow-up task if any would refuse** | They catch documentation and registry drift, not user-facing defects. That should not hold a hotfix, and the existing return path (`merge_close.rs:156`) still applies. |
| Bug case G3 | **Defer to the receipt** | It catches recurrence. The receipt requires the case, or a `no case:` decision, before close. |
| Verifier | Keep, post-merge (as today) | It catches missed acceptance criteria. It already runs after merge for parked lanes, so it does not hold the merge. |
| Proof claims, MERGE REQUIRED, unpushed tip | Keep | These are integrity guards that cost seconds. |
| Pre-PR before-screenshots | **Move off the critical path** | They are evidence capture, not a check. Capture them in parallel after the PR opens. |

## Post-merge receipt and its enforcement

At merge, `worktree_merge_checked` (`worktree_ops.rs:4214`) records the
admission as Green with reason `fast-lane`. It then arms a per-task receipt.
This uses the same arming point `qa_post_release` uses for epics today
(`worktree_ops.rs:4673`), narrowed to one task, with a 30-minute window.

The receipt passes when all three of these hold on the merge commit:

1. The `e2e-hermetic` run is green. Cassy waits on it through the existing
   `ci_completed` reminder filter.
2. The Vercel production status is success.
3. A QA ledger row walks each touched journey on production, or the task
   records `no journey impact:` when the diff touches none.

The task cannot close until the receipt passes. That closes the gap that left
cas-cc61 open after its merge.

On failure:

- **e2e red on the merge commit.** Cassy reruns the failing spec once. If it is
  still red and the same spec is green on the merge's first parent, Cassy opens
  a revert PR (`git revert -m 1 <merge>`). The revert is itself fast-lane
  eligible: same paths, same size. It merges on Frontend unit tests, the
  operator is told, and the task gets `request_changes` (`merge_close.rs:695`).
  If the parent is also red, the failure predates the hotfix. Cassy files a
  follow-up task and does not revert.
- **Vercel build failed.** Production is unchanged. Cassy sends
  `request_changes` with the build log; no revert is needed.
- **QA walk FAIL.** Cassy asks the operator to choose between revert and keep
  and fix forward. After 15 minutes, the default is revert.

## Time budget

Request to preview needs pulse-card to allow PR previews for eligible
branches, by changing the Ignored Build Step rule. No preview build has been
measured yet. The 3:45 production build is used as a stand-in until the first
fast-lane run gives a real number.

| Step | Before (measured) | After target | Change |
| --- | --- | --- | --- |
| Task created to worker started | 1:25 | 1:25 | none |
| Read and edit | 2:05 | 2:05 | none |
| Local format, typecheck, scoped vitest | 0:45 | 0:45 | kept |
| Push to PR open | 1:54 (plus 1:29 of screenshots first) | 0:10 | PR opens at push; screenshots run in parallel |
| **Request to preview** | no preview exists | **about 8:10** (preview build 3:45, stand-in) | previews on |
| PR CI to merge gate | 11:09 median to all green | 4:50 (Frontend unit tests) | backend skipped; e2e and fresh build deferred |
| Gate green to merge | 0:36 | 0:36 | none |
| Merge to production | 4:06 | 4:06 | none |
| **Request to production** | about 20:08 by the gates (15:37 today, by skipping them) | **about 13:57** | |
| Post-merge receipt | none | about 11 minutes after merge (e2e on main), plus the journey walk | new, off the critical path |
| Supervisor overrides per hotfix | several (cas-f426) | 0 | |

## Governance: plan-fit and plan-target

Neither gate is on main. PLAN-FIT and "plan target met" exist only on the
unmerged governance branch for epic cas-6e58, as task-verifier prompt rules and
supervisor guidance, with no Rust enforcement. When they land:

- Eligibility rule 5 keeps phase work out of the lane, so a hotfix is never
  counted toward a plan target.
- The verifier's PLAN-FIT check treats a fast-lane task as "outside the plan",
  not as a plan miss.
- A hotfix that turns out to need phase work fails eligibility at park and
  takes the normal path. It is not split silently.

## What this removes from today's path

- Backend CI on frontend-only PRs: about 14 runner-minutes per PR (job medians 1:59, 2:44,
  2:41, 1:10 and 5:30 for boot, typecheck, unit, fresh DB and contracts).
- A second hermetic e2e run per change: the PR run goes, and the on-main run
  becomes the receipt.
- The push-to-PR idle gap and the screenshots that sat on the critical path.
- The pre-merge QA bundle and its staging-SHA batch receipt for these
  deliveries, and with them the supervisor overrides.
- The unrecorded "merge before green" judgment call: it becomes a rule with a
  receipt and a revert.

## What it adds

- One project config table, `[fast_lane]`, with `paths`, `deny`, `max_files`,
  `max_lines`, `merge_checks`, `receipt_checks` and `receipt_window_mins`.
- An eligibility decision at park.
- A per-task receipt that reuses the post-release arming.
- Revert-PR creation on an attributable failure.

No new pre-merge check stage is added.

## Rejected alternatives

- **Merge on the worker's local 0:45 run.** Production would be about 9
  minutes from request. But the receipt is worker-written, so nothing
  independent would gate production.
- **Keep e2e as a pre-merge gate and only parallelise.** It is the 10:43 long
  pole, so the 10-minute preview target is still missed.
- **Widen the operator rapid-response window.** It is time-boxed and armed by
  hand. Daniel would have to remember to open it mid-demo.

## Recommendation and the decision for Daniel

Recommendation: approve the fast lane as designed.

Eligible frontend-only hotfixes are those up to 10 files and 300 lines,
inside `apps/frontend/app/src`, locales or tests, and outside the API, auth,
billing, middleware and config paths. They merge on CI Frontend unit tests
(vitest plus vue-tsc, 4:50 measured). Hermetic e2e, the production build and
the journey QA walk become a 30-minute post-merge receipt that blocks close. An
e2e failure caused by the hotfix opens a revert automatically.

With PR previews switched on in pulse-card's Vercel project, the target is
about 8 minutes from request to preview and about 14 minutes to production,
with no supervisor overrides.

Decision: approve this lane, including automatic revert on an attributable
post-merge e2e failure, so the build can be staffed under epic cas-ae20.

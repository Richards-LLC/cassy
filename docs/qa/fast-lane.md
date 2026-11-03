# Hotfix fast lane

A small frontend-only hotfix merges as soon as its unit tests and typecheck
pass. The slower checks run after the merge and must pass within 30 minutes
before the task can close. Design: `docs/design/frontend-hotfix-fast-lane.brief.md`.

## Configuration

A project opts in in `.cas/config.toml`. The lane stays off until `enabled`,
`paths` and `merge_checks` are all set.

```toml
[fast_lane]
enabled = true
paths = ["apps/frontend/app/src/**", "apps/frontend/locales/**", "apps/frontend/test/**", "docs/qa/journeys.yaml"]
deny = [
  "apps/frontend/server/**", "apps/frontend/middleware/**", "apps/frontend/app/src/middleware/**",
  "**/*auth*", "**/*auth*/**", "**/*Auth*", "**/*Auth*/**", "**/*billing*", "**/*billing*/**", "**/*stripe*", "**/*stripe*/**",
  "**/*payment*", "**/*payment*/**", "**/package.json", "**/package-lock.json", "**/pnpm-lock.yaml", "**/yarn.lock", "**/nuxt.config.ts",
  "**/vercel.json", "**/.env*", ".github/**", "apps/backend/**",
]
max_files = 10
max_lines = 300
merge_checks = ["PR gate"]
receipt_checks = ["Hermetic e2e", "Frontend fresh build"]
receipt_statuses = ["Vercel"]
receipt_window_mins = 30
```

Each key can also be set with `cas config set fast_lane.<key> <value>`. List
values are comma-separated.

Check and status names match exactly and are case-sensitive:

- `merge_checks` and `receipt_checks` are GitHub check-run names: the job's
  `name:` in Actions, or the job id when it has no name. A job called from a
  reusable workflow appears as `<caller> / <job>`; use that whole string.
- `receipt_statuses` are commit-status contexts on the merge commit, such as
  `Vercel` for the production build.
- Workflow names are not read.
- Path globs are case-sensitive, so list both spellings where they differ, such as `*auth*` and `*Auth*`.

## Eligibility

Cassy measures eligibility at park from the delivered diff against its merge
base with the target, and again at merge. Every rule must hold:

- Every changed path matches `paths`, and none matches `deny`.
- The diff has at most `max_files` files and `max_lines` changed lines (added
  plus deleted).
- The delivery is not Tier 2. Guard, CI, release and close-gate code never
  rides the lane.
- The task is a bug, a review finding, or labelled `hotfix`. Epic, plan and
  phase work never rides the lane.

The result is a decision note on the task, either
`FAST LANE ELIGIBLE head=<sha> files=N lines=M` or
`FAST LANE REFUSED head=<sha>: <reason>`. A refused delivery takes the normal
path.

## What changes for an eligible delivery

| Stage | Normal path | Fast lane |
| --- | --- | --- |
| Park: upkeep G1, G2, G5, G6 | refuse | a notice, plus one follow-up task |
| Park: bug case G3 | refuse | deferred; enforced at the post-merge close |
| Park: QA evidence | refuse without evidence | deferred to the receipt |
| Merge CI | every check run on the tip | `merge_checks` only |
| Close after merge | QA evidence gate and independent QA | the post-merge receipt |
| Verifier, proof claims, MERGE REQUIRED | kept | kept |

## The post-merge receipt

`worktree_merge` records `FAST LANE MERGE head=<sha> merge=<sha> at=<time>`.
Cassy's close after the merge then requires:

1. every `receipt_checks` check run to be green on the merge commit;
2. every `receipt_statuses` status to have succeeded on the merge commit;
3. a QA ledger row for each touched journey, walked on production, in the
   task's `LEDGER.md`; or a decision `no journey impact: <reason>` when the
   diff touches no journey.

While checks are still running, the close waits. Cassy sets a `ci_completed`
reminder on the merge commit, and the close is retried when CI finishes. A
receipt still pending after `receipt_window_mins` is reported as overdue.

On failure:

- **A receipt check fails.** Cassy reruns its failed jobs once. If the check
  fails again and passed on the merge's first parent, the hotfix caused the
  failure. Cassy opens a revert PR from `fast-lane-revert/<task>`
  (`git revert -m 1 <merge>`) and returns the task to its worker. The
  supervisor merges the revert once its merge checks pass and tells the
  operator.
- **The check also failed on the first parent.** The failure predates the
  hotfix. Cassy files one follow-up task and does not revert.
- **A receipt status fails**, for example the Vercel build. Production keeps
  its previous deployment. The task returns to its worker with no revert.
- **A ledger row is FAIL.** The task returns to its worker, and the
  supervisor asks the operator whether to revert or fix forward.

## Worker guidance

- Push and open the PR in the same step. CI starts only when the PR opens.
- Take before and after screenshots in parallel with CI. They are evidence,
  not a check, and must not hold up the PR.
- Run the project's local format, typecheck and scoped unit tests as
  feedback. The merge gate is the CI run.
- After the merge, walk each touched journey on production and add one
  ledger row per journey. If the diff touches no journey, record the
  decision `no journey impact: <reason>`.

## Steps outside Cassy (pulse-card)

- **PR previews (operator, Vercel dashboard).** The pulse-card Vercel project
  cancels every PR build with its Ignored Build Step. Previews for lane
  branches need that rule changed in the dashboard. Cassy cannot do this; it
  is the operator's step.
- **Backend CI path filters (pulse-card repository).** Add path filters so
  frontend-only PRs skip the backend boot, typecheck, unit, fresh-database and
  PostgreSQL-contract jobs. Expose a single always-reporting `PR gate` job on
  PRs and a `Post-merge receipt` workflow on push to `main`. That change is made
  in the pulse-card repository, coordinated through the supervisor.

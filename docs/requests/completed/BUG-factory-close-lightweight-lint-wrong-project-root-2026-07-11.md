---
from: Ozer Health factory (cas-lint-relay worker relay)
date: 2026-07-11
priority: P1
type: BUG
component: factory / task-close / lightweight structural lint
project: ozer-health (Richards-LLC/ozer-health)
for_team: cas-src
cas_task: cas-d1d2
---

# BUG: factory worker close runs lightweight structural lint against the wrong project root

**Label:** `factory` · `task-close` · `lightweight-lint` · `wrong-root` · `worktree` · **P1**

Please treat this as a **cas-src factory close-gate** bug. The downstream project code under lint is unrelated WIP on the main checkout, not the worker's task deliverables.

## Summary

When a factory worker closes a task in supervisor-owned review mode (`[code_review] owner = "supervisor"`), the lightweight structural lint gate runs `git diff HEAD` in `close_project_root`, which is resolved as `cas_root.parent()` — the **main project checkout** — instead of the worker's isolated git worktree or the worker branch's committed diff.

If the main checkout has unrelated uncommitted edits (common during active factory sessions), the lint falsely fails on violations that do not belong to the task being closed. Sibling close gates (`has_reviewable_changes`, additive-only, uncommitted-work) were already fixed to prefer the worker worktree (`cas-ee2b`, `cas-bc1b`); lightweight lint was not updated and still scans the wrong tree.

## Expected behavior

- Lightweight structural lint should inspect **only the worker's task-scoped diff** — the committed changes on the worker branch (e.g. `merge-base..HEAD` inside the resolved worker worktree), matching how `has_worker_committed_reviewable_changes` and `check_additive_only_branch_violations` already scope other close gates.
- A worker whose isolated worktree is clean (`git diff HEAD` empty) and whose commits are merged into the epic parent branch should pass lint and proceed to `PendingSupervisorReview` (or close, depending on depth).
- Unrelated dirty files in the main checkout (supervisor WIP, another feature branch checked out at the repo root, etc.) must not affect worker close.

## Actual behavior

- `run_lightweight_structural_lint(close_project_root)` is called with `close_project_root = self.cas_root.parent()` (`close_ops.rs` ~L1333, ~L1457).
- The lint collects diff text via `git diff --unified=0 HEAD` in that directory, which includes **all uncommitted changes** in the main checkout's working tree.
- Worker close fails with `⚠️ LIGHTWEIGHT LINT FAILED` reporting consecutive `//` comment violations from files the worker never touched, while the worker worktree itself has an empty `git diff HEAD`.

## Reproduction

### Preconditions

1. A factory project with isolated worker worktrees under `.cas/worktrees/<name>` (standard Cassy factory layout: `cas_root` = `<repo>/.cas`, parent = repo root).
2. Main project checkout at the repo root on branch `staging` (or any branch) with **unrelated dirty tracked files** — e.g. WIP on `apps/frontend/pages/home.vue` and `apps/frontend/pages/home.spec.ts` for a coordinated-tile-reveal feature (`cas-387d` follow-up) that is **not** part of the worker's epic.
3. A worker isolated worktree on `factory/<worker>` with task commits pushed and merged into the epic branch; worker worktree `git diff HEAD` is empty.

### Steps

1. Worker completes task work in isolated worktree `factory/web-bundle-checkout` (example), commits, pushes.
2. Supervisor merges `factory/web-bundle-checkout` into epic branch `epic/<epic-slug>` (merge commit lands; `git merge-base --is-ancestor <worker-HEAD> <epic-branch>` succeeds; worker branch has 0 commits ahead of epic).
3. Worker calls `cas__task action=close` for the task.
4. Merge gate may initially park the task as `awaiting_merge` (expected). After supervisor merge, worker retries close.
5. **Observe:** close is rejected by lightweight structural lint with violations at lines `+91–+97` and `+168–+174` — consecutive `//` comment blocks.

### Control (proves wrong root)

| Location | `git diff HEAD` | Lint-relevant? |
|---|---|---|
| Main checkout (`/home/pippenz/Petrastella/ozer`, branch `staging`) | Dirty: `home.vue` (+114 lines), `home.spec.ts` (+57 lines) | **Yes — this is what lint scans today** |
| Worker worktree (`.cas/worktrees/web-bundle-checkout`, `factory/web-bundle-checkout` @ `fd541aa9`) | Empty (only unrelated untracked `.husky/_/`) | **No — this is what lint should scan** |
| Epic worktree (`.cas/worktrees/epic-cas-9222`, epic branch @ `252fdc25`) | Clean | N/A |

### Simulated lint (reproduces reported line numbers)

Running the lint's consecutive-`//` heuristic against `git diff --unified=0 HEAD` in the **main staging checkout** (not the worker worktree) yields exactly the failures reported by Cassy close:

```
Violation: Lines +91–+97: 7 consecutive // lines (in apps/frontend/pages/home.spec.ts)
Violation: Lines +168–+174: 7 consecutive // lines (in apps/frontend/pages/home.vue)
```

Running the same simulation in the worker worktree returns **zero violations** (empty diff).

## Concrete evidence (downstream tasks)

Two tasks in epic `cas-9222` (Enable Stripe bundle checkout on the web) hit this defect on **2026-07-11** after their work was merged:

### cas-0262 — Wire bundle detail to Stripe saved-card checkout on web

- Worker branch: `factory/web-bundle-checkout` @ `9c7e6bff` (later `fd541aa9` after follow-up commits).
- Worker worktree: `git diff HEAD` empty; focused tests passed (`sku.spec.ts`, `useIntentCheckout.spec.ts`, `checkout-payment.spec.ts` — 52 tests).
- Merged into epic; close retried after merge.
- **Blocked:** `LIGHTWEIGHT LINT` with `+91–97` and `+168–174` consecutive `//` comments.
- Worker note: *"Those lines are UNRELATED dirty uncommitted work on main ozer checkout (staging: home.vue coordinated tile reveal), not factory/web-bundle-checkout."*

### cas-ac4b — Remove Curative-friendly from bundle trust descriptions

- Same worker branch/worktree; commit `fd541aa9`; `sku.spec.ts` — 26 passed.
- Merged into epic (`252fdc25`); close retried.
- **Blocked:** identical lint failure (`+91–97` / `+168–174`) — same wrong-root scan of main staging dirty `home.vue`, not ac4b deliverables.

### Violation source files (main checkout only)

The reported line ranges correspond to **added lines** in the main-checkout diff for coordinated-tile-reveal WIP:

**`apps/frontend/pages/home.spec.ts`** — block comment above the skeleton-grid describe block (7 consecutive `//` lines in the unified added-lines stream → lint positions `+91–+97`):

```ts
// ── Coordinated tile reveal — skeleton grid until every async gate settles ─
//
// Tiles used to pop in one-by-one as their individual async signals
// (PostHog flags, telehealth flag, dashboard-messages probe, bundles
// catalog, credits balance) resolved, reflowing the grid for seconds.
// Contract: a skeleton grid holds layout until `tilesReady`, then all
// tiles mount in the same tick; a hard timeout caps skeleton time.
```

**`apps/frontend/pages/home.vue`** — script-section block comment for coordinated tile reveal (7 consecutive `//` lines → lint positions `+168–+174` in the flat added-lines stream across all dirty main-checkout files):

```ts
// ── Coordinated tile reveal (cas-387d follow-up) ──────────────────────────
// Each dashboard tile is gated by its own async signal (PostHog flags,
// telehealth flag, dashboard-messages probe, bundles catalog, credits
// balance), so tiles used to pop in one-by-one and reflow the grid for
// seconds after first paint. Instead: hold a skeleton grid until EVERY
// gating signal has settled, then mount all tiles in the same tick so the
// shared entrance animation plays once, together.
```

Both violation ranges are indexed against the **aggregate** `git diff HEAD` of the main checkout, not against any file the bundle-checkout worker touched.

None of these files appear in the cas-0262 or cas-ac4b task file lists (`apps/frontend/pages/bundles/[sku].vue`, composables, utils only).

## Impact

- **False close failures** after legitimate merge: workers cannot reach `PendingSupervisorReview` despite clean worktrees and integrated commits.
- **Supervisor/worker retry churn:** both cas-0262 and cas-ac4b parked `awaiting_merge`, merged, then blocked again on lint — requiring supervisor diagnosis and a cross-team bug relay (this file).
- **Encourages bad workarounds:** stashing or reverting unrelated main-checkout WIP to unblock unrelated worker closes; or `bypass_close_gates` — both mask the root cause.
- **Regression class:** same family as cas-ee2b (reviewable-changes false positive on main dirty tree) and cas-bc1b (additive-only scanning main instead of worker branch). Lightweight lint is the remaining un-fixed caller of `close_project_root` for isolated workers.

## Likely fix direction

In `close_ops.rs`, resolve the lint `project_root` the same way other worker-aware gates do:

1. **Preferred:** When `resolve_worker_worktree_path(&task)` returns `Some(worker_wt)`, pass `worker_wt` to `run_lightweight_structural_lint` (or a new helper that diffs `merge-base(parent)..HEAD` inside the worktree, consistent with `has_worker_committed_reviewable_changes`).
2. **Alternative:** Extend `run_lightweight_structural_lint` to accept an optional worktree path + parent branch and use committed-range diff (`git diff --unified=0 <merge-base>..HEAD`) instead of working-tree `git diff HEAD`.
3. **Do not** continue using `cas_root.parent()` for isolated factory workers — that path is the shared main checkout and is intentionally dirty during parallel factory work.

### Code pointers (cas-src)

| Location | Issue |
|---|---|
| `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs` L1333 | `close_project_root = self.cas_root.parent()` |
| `close_ops.rs` L1457 | `run_lightweight_structural_lint(close_project_root)` — should use worker worktree when resolved |
| `close_ops.rs` L1353–1368 | `has_worker_committed_reviewable_changes` — **already** worker-scoped; lint should follow |
| `close_ops.rs` L4320–4348 | `run_lightweight_structural_lint` uses `git diff HEAD` in `project_root` |
| `verification_flow.rs` `test_additive_only_uses_worker_branch_not_main_worktree` | Existing regression test pattern for the same wrong-root family (cas-bc1b) |

### Suggested regression test

Mirror `test_additive_only_uses_worker_branch_not_main_worktree`:

- Real git repo + isolated worker worktree with a clean committed task diff (passes lint).
- Dirties an unrelated tracked file in `cas_root.parent()` with >5 consecutive `//` lines.
- Assert worker close lint **passes** (post-fix) or **fails today** (pre-fix baseline).

## Notes

- Reporting-only relay. No Cassy source was modified to produce this file.
- Downstream identifiers (ozer, cas-0262, cas-ac4b, branch names) are included because they are the concrete reproduction; Cassy maintainers can inspect the live worktrees listed above.
- Workaround applied downstream: tasks left in `awaiting_merge` / blocked on lint; no main-checkout stash was performed (to avoid losing unrelated WIP and to preserve repro state).


## Resolved (cas-dc5d)

**Root cause:** `run_lightweight_structural_lint(close_project_root)` was always
invoked with `close_project_root = cas_root.parent()` (shared main checkout) and
`git diff HEAD` (working-tree WIP). Sibling gates (cas-ee2b / cas-bc1b) already
scoped to the isolated worker worktree; lint did not.

**Fix:**
1. Call site in `cas_task_close` supervisor-review block: when
   `resolve_worker_worktree_path` returns `Some(worker_wt)`, run
   `run_lightweight_structural_lint_with_scope(worker_wt, Some(parent_branch))`.
2. Scoped mode diffs `merge-base(HEAD, parent)..HEAD` inside the worker
   worktree — task-committed range only; main-checkout dirty files are never
   visible.
3. Non-isolated closes keep legacy working-tree `git diff HEAD` via
   `run_lightweight_structural_lint(close_project_root)`.
4. **P1 parent authority:** `worker_review_parent_branch` uses the same
   `get_parent_epic(...).branch` → `resolve_standalone_merge_target` path as
   the merge-state gate (not `task.worktree_id` / hard-coded `main`). System-B
   workers rarely have `worktree_id`.
5. **P2 fail-closed range proof:** unsafe/missing parent, failed merge-base,
   or failed `git diff` → `LightweightLintOutcome::Fail` with actionable text
   (never silent Pass).

**Regression (`lightweight_lint_tests`):**
- `lint_scoped_to_worker_range_ignores_main_checkout_wip`
- `lint_scoped_to_worker_range_fails_on_committed_todo`
- `lint_scoped_parent_must_be_epic_not_main_when_they_diverge` (P1)
- `lint_scoped_fails_closed_on_missing_parent` / `_unsafe_parent` /
  `_merge_base_failure` (P2)

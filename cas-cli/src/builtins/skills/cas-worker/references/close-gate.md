---
name: close-gate
description: Worker pre-close self-verification gate.
managed_by: cas
---

# Close Gate — Self-Verification

Run all 6 self-verification checks before `mcp__cas__task action=close`. They apply to every task type — but a few types add a gate on top of them (see "Task-type extras" below), so the close path is not identical everywhere. Skip the checks and you eat a verifier rejection round-trip.

## Depth: `light` tasks skip this gate

If the task you're closing is `depth=light` (check the `Depth:` line in `task show`), **skip the 6 pre-close self-checks below** and close once it runs on localhost. Light is speed mode: minimal diff, no gold-plating, the human is the evaluator — there is no DoD to chase. You must still not claim a proof you didn't run. For `depth=deep` or unset, run the full gate below.

## Check 0 — merge state (the rejection you WILL hit)

Close runs a merge-state guard before anything else: every commit on your `factory/<name>` branch must already be on the task's parent branch, or close rejects with `⚠️ MERGE REQUIRED`. This is a **data-state guard** — `bypass_code_review=true` does not skip it, and neither can the supervisor. Get merged *before* attempting close:

- **Parent is `epic/<slug>`** — epic branches are supervisor-local by convention. Run `mcp__cas__coordination action=inbox_poll` **repeatedly until it returns `No unread messages`** to drain unread supervisor messages — one poll returns at most 10 rows, so a single call is not a freshness check — then capture the current tip with `git rev-parse factory/<name>`, then push and send a freshness-qualified merge request only if one is still needed. **Never `gh pr create --base epic/...`** — that ref doesn't exist on origin; the call always fails.
- **Parent is `main`/`master`/`staging`** — push and complete the project's PR/merge flow (or the merge flow the supervisor stated at assignment), then close.
- **Guard still fires after a confirmed merge?** Squash-merges rewrite SHAs, so the guard can count already-merged commits as missing. You can usually clear this yourself: re-close with `commit_receipt=<sha>` naming the commit that carries your work. A receipt that resolves to a commit with a **non-empty diff** that is already **reachable from the parent branch** (or `origin/<parent>`) IS the delivery evidence, and close proceeds with a note even if the lane branch still carries unrelated residue (cas-e74c). If the receipt is rejected, send the supervisor the exact guard text plus the rejection reason — they fix the stale branch ref. Do not retry-loop.

**Never bypass the close path.** Setting `status=closed` via `action=update` and hand-writing a `mcp__cas__verification action=add` record forges the verification audit trail — the task looks verified when nobody verified it. If close keeps rejecting, that is a supervisor conversation, not a workaround opportunity.

**Code review is not your job at close** under the v2.13.0+ default `[code_review] owner = "supervisor"`. The supervisor runs a lightweight gate when merging each worker diff, then one full `/cas-code-review` pass when the epic is code-complete. Do not invoke the multi-persona review yourself unless your supervisor explicitly tells you to, or your project has opted in to legacy `owner = "worker"` in `.cas/config.toml` — the worker-inline path adds ~14 min and ~100K tokens per close, which is exactly what the v2.13.0 flip was designed to eliminate. This is enforced, not advisory (cas-4fef): under `owner = "supervisor"` a worker dispatching `cas-code-review` is refused at the dispatch layer. The close path does not want an envelope from you either — attempt the close WITHOUT `code_review_findings` and let it transition the task to PendingSupervisorReview. Producing findings to "close properly" is the exact mistake this rule exists to stop.

## Delivery receipts (worker-supplied close evidence)

Two distinct `close` fields — you supply them, no supervisor needed:

- **`commit_receipt=<sha>`** — full SHA or unambiguous abbreviation of the commit this task delivered. CAS resolves it to an immutable commit id, validates a **non-empty diff** and **ancestry from the parent branch**, and persists only the resolved id. This is the self-serve remedy for squash-merge SHA drift, and for cherry-picked delivery out of a lane branch that still carries other tasks' commits: a valid receipt clears the merge-state guard and close proceeds with a note about the lane's residue (cas-e74c). If the receipt is rejected, close tells you why — fix that, don't retry blind.
- **`completion_receipt=<json>`** — opt-in transactional close. Serialize a `WorkerCompletionReceiptInput`: `{task_id, worker_agent_id, repo_selector, source_branch, commit_sha, merge_base_sha, target_branch, target_sha, proof_reference, scope_summary}`. CAS revalidates every identity-bearing field against registered agent state and live Git before persisting an immutable delivery transaction, then releases your lease and moves the task to `pending_supervisor_review` awaiting a fresh verification verdict. Omit it and the legacy close path is unchanged. Rejection returns `DELIVERY RECEIPT REJECTED` and changes nothing.

## Task-type extras

The 6 checks below apply to every task type. These gates sit on top of them:

- **Spike** (`task_type=spike`) — optional `search_manifest`: a JSON array of the search steps you ran, e.g. `[{"command": "rg -c foo src/", "hits": 3}]`. **Opt-in and warning-only** — it never blocks close. Entries with `hits: 0` (or a manifest that fails to parse) are appended to the task as a loud `ZERO_HIT_SEARCH_WARNING` note, because a search that matches nothing anywhere is more often a broken pattern than a clean corpus (cas-49f1). Supply it whenever your conclusion rests on "I searched and found nothing".
- **Epic** — closing an epic additionally walks its children and blocks on any child whose recorded work is not merged into the parent branch, not just on child status.

## Pre-Close Self-Verification

### 1. No shortcut markers
```bash
# Must return zero UNEXPLAINED results in your changed lines
git diff -U0 <base> | rg 'TODO|FIXME|XXX|HACK'
git diff -U0 <base> | rg 'for now|temporarily|placeholder|stub|workaround'
```

Triage hits, don't grind: scope to your changed lines (as above), and legit occurrences — UI `placeholder=` attributes, pre-existing comments in untouched lines, test-fixture names — don't block close. Record a one-line justification for anything you leave; only new, unexplained markers are failures.

Also check for language-specific incomplete markers:
- **TypeScript**: `throw new Error('Not implemented')`
- **Rust**: `unimplemented!()`, `todo!()`
- **Python**: `raise NotImplementedError`

### 2. All new code is wired up
For every new function, class, module, route, or handler you created:
```bash
# Verify it's actually called/imported somewhere outside its definition
rg 'your_new_symbol'            # repo-wide; scope to source dirs in monorepos, e.g. apps/ packages/ src/
```
If zero external references → you built it but didn't wire it in. Fix before closing.

Registration checklist (varies by framework):
- New CLI command → added to command registry?
- New API route/endpoint → added to router or module?
- New migration → listed in migration runner?
- New service/provider → registered in DI container?
- New config field → has a default, is read somewhere?

### 3. Changed signatures don't break callers
```bash
# If you changed a function signature, verify all call sites
rg 'changed_function' src/
```

**Rust public-type additions:** adding a field to a `pub struct` is a silent breaking change for downstream crates that construct the struct by listing every field (no `..Default::default()`). Those crates compile fine in isolation but fail with `E0063` at workspace scope. Always verify with check 4 scope rules when touching public types.

### 4. Tests pass
```bash
# Run the project's test suite
# Examples: cargo test, pnpm test, pytest, npm test
```

If tests fail in code you didn't modify:
1. Re-run to check if flaky (transient failures happen).
2. If consistent, report as blocker with the specific test name and error output.
3. Do NOT try to fix other people's tests — that's out of scope.

#### Rust: per-crate vs workspace-wide test scope

| What you changed | Minimum test run |
|---|---|
| Internal logic, private functions only | `cargo test -p <crate>` |
| Public type in `crates/*/src/lib.rs` — new/removed field, changed signature | **`cargo test --workspace`** |
| Anything in `crates/cas-mux`, `crates/cas-factory`, `crates/cas-types` | **`cargo test --workspace`** (consumed by `cas-cli`) |
| Test files only (`tests/**/*.rs`) with no API change | `cargo build --workspace --tests` at minimum |

**Why per-crate isn't enough:** when you add a field to a `pub struct` in a shared crate, that crate's own tests pass (the new field has a `Default`). But downstream crates that name every field in a struct literal fail with `E0063`. `cargo test -p <crate>` never sees this — only `cargo test --workspace` or `cargo build --workspace --tests` catches it.

**JS/TS monorepos (pnpm/turbo):** same blast-radius logic. Changed a shared package or exported type → run the *consuming* apps' typecheck and tests (`pnpm -r typecheck`, `pnpm --filter <app> test`), not just the package's own suite. A changed interface compiles fine in its own package and breaks only where it's consumed.

**Historical note (cas-c0e0):** two fields were added to `FactoryConfig` (cas-factory). Per-crate tests passed. `cas-cli` constructors failed E0063 at workspace scope. The regression shipped to main as commit `3dc7488` and was caught only during manual merge.

### 5. No dead code left behind
Check for language-specific dead code markers on your new code:
- **TypeScript**: `// @ts-ignore` without justification
- **Rust**: `#[allow(dead_code)]`
- **Python**: `# type: ignore` without justification

### 6. System-wide test check

For every non-trivial change, trace **2 levels out** from the edited code — callers of the edited symbols, observers/middleware, hook subscribers, anything that imports the edited module. For each touched boundary:

- Confirm integration tests exist for that boundary, with **real objects** (not mocks) at the crossing point.
- **Run those integration tests** — not just the file you edited. `cargo test <crate>::<integration-test>` or equivalent. Presence of a test file is weak signal; an executed test is evidence.

"2 levels out" is LLM-judgment — do not over-engineer this into a call-graph analysis. Read the code, identify the obvious boundaries, test them.

**Skip allowed for**: pure additive helpers with no callers yet, pure styling changes, pure documentation changes. If you skip, record *why* in a task note (`note_type=decision`) before close. Don't skip silently.

Only close after all checks pass. The verifier will catch what you miss — but rejections cost time.

## Simplify-As-You-Go

After closing your **third** task in the current EPIC — and again after the 6th, 9th, 12th, etc. — invoke the `simplify` skill on your own recent work in that EPIC before picking up the next task.

- **Counter is per-worker-per-EPIC.** It resets when you move to a different EPIC.
- **Counter is stateless** — derive it at close time by querying `mcp__cas__task action=list assignee=<self> epic=<current-epic> status=closed` and checking whether `(count + 1) % 3 == 0` (the `+1` is for the task you're about to close).
- **Scope of simplification** = your own committed and staged work within the current EPIC only. Not cross-worker. Not cross-EPIC. Not code you haven't touched.
- **If the EPIC has fewer than 3 of your tasks total**, simplify-as-you-go never fires for you in that EPIC. That is intentional — the trigger exists to catch pattern accumulation, and <3 tasks is below the accumulation threshold.

The simplify pass should produce visible output — a commit, a task note, or an explicit "nothing to simplify" decision note. Do not run it silently.

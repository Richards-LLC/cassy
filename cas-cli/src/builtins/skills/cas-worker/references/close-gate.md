---
metadata:
  managed_by: cas
---

# Close Gate — Delivery

Read at delivery. Review owns coding and test-quality judgments; this branch
collects evidence and delivers the assigned task.

## Commit and push

Commit scoped work; push once before parking, once per review round, unless
`delivery_mode=local_merge`. Each push starts CI. The pre-push hook runs
`scripts/fast-rows-pre-push.sh` when present; missing tools fail the push.
CI owns fast-row proof; local receipts are pre-push feedback. Park reads
exact-tip capped test receipts, never runs checks. For changelog scope and
format, follow [Changelog at park](#changelog-at-park).

## Clean-tree receipt

Run `git status --porcelain` and `git rev-parse HEAD`. Empty porcelain and HEAD
equal to the delivered SHA establish what is on disk. A rejected write may
still have changed disk: inspect any divergence, commit your authorized work,
and record its disposition before close. Escalate unexplained writes instead
of closing over them. This receipt applies at every depth.

For `depth=light`, deliver the minimal working outcome with honest evidence.
For deep or unset depth, map every criterion to executed proof or a named proof
owner using `verify-before-claim`; trace affected consumers and registrations.
Consume the receipts from [discipline.md](discipline.md); name unexecuted
obligations and their assembly owner.

## Ledger preflight

Create required evidence with `cas qa ledger init --task <id>`; it prints the
project-scoped path and preserves existing evidence. Fill it from the real
walk, then run `cas qa ledger check <path> --task <id>`. Fix every line/row
diagnostic. Close uses that check before verification dispatch, including
existing no-code ledgers. A structural PASS does not judge captures.

## Merge and freshness

Close requires the delivered work on the named target. On MERGE REQUIRED:

1. Drain `coordination action=inbox_poll` to `No unread messages`; act on
   unread supervisor messages and re-read the task before corrective edits.
2. Capture the current tip: `git rev-parse factory/<name>` (use your per-task
   branch if active). Check `git merge-base --is-ancestor <delivered-tip> <target-tip>`.
   If already merged or closed, stop and await the verdict rather than amend stale state.
3. For `delivery_mode=local_merge`, retain the local commit and request supervisor
   merge with its branch and SHA. Push only with explicit supervisor authorization.
   Otherwise push the delivered branch, then send `merge_request=true` with the
   target and SHA. The merge closes the task: Cassy dispatches verification at
   the merge, a PASS closes it and tells you once, a FAIL reopens it to you with
   findings. Do not re-close or block on that verification. While it is pending, follow
   [recovery.md](recovery.md#while-the-merge-is-pending): no polling loop.
4. For a protected target, follow its PR flow. Ask for an epic merge rather than
   `gh pr create --base epic/...`; the supervisor owns the merge venue.

If another assigned task follows a handed-off delivery, start its per-task branch
from the epic tip (`factory/<name>-<task-id>`), preserving the parked branch.
No simultaneous task ownership. For rejection details, read [recovery.md](recovery.md).
Never set `status=closed` or invent a verification record to bypass close.

## Upkeep refusals at park

Before the merge request, close checks that the delivery kept the journey
registry, `.claude/CODEMAP.md` and `docs/PRODUCT_OVERVIEW.md` true for the
paths it changed (G1–G3, G5, G6). Old debt never refuses; it prints as a
warning. On a refusal, fix it in this branch and commit (own the new file,
add the bug's case, edit the named CODEMAP or overview rows), then retry
close. When the change truly needs no upkeep, record the gate's escape token
from cas-qa-craft's
[escape vocabulary](../../cas-qa-craft/references/journeys.md#escape-vocabulary)
as a decision note with a specific reason, then retry. Cassy writes the
`UPKEEP ESCAPE` and `UPKEEP OBSERVE` notes itself; never write them by hand,
and treat an `UPKEEP OBSERVE` note as the refusal a later `enforce` stage will
make. The reviewer and QA see each escape, so a vague reason costs a round.

## Changelog at park

For a delivery that changes shipped Rust, `hub-web/src`, user-run scripts or
builtin skills, add or extend a titled entry in `CHANGELOG.md` under
`## [Unreleased]`, grouped under a category and product area (for example
`### Fixed — Commander` with a per-delivery `- **Title** — description` line).
If the delivery is test-only, docs-only or internal QA, record a typed
decision note with `no changelog: <reason>`. The close gate checks that the
task's own delivery changed an Unreleased entry; the changelog fast row
checks the entry format.

## Delivery receipts

- `commit_receipt=<sha>` names this task's immutable, non-empty delivered commit.
  Close validates ancestry against the target; use it for squash/cherry-pick
  identity drift. Forward a rejected receipt with the exact guard error.
- `completion_receipt=<json>` requests transactional close after the current
  source tip is merged. Fields: `task_id`, `worker_agent_id`, `repo_selector`,
  `source_branch`, `commit_sha`, `merge_base_sha`, `target_branch`, `target_sha`,
  `proof_reference`, `scope_summary`, optional `artifact_path`. The artifact must
  exist under the configured task artifacts directory. Identity and Git are
  revalidated; verification remains supervisor-owned.

## Conditional evidence

- Demo, configured user-facing path or a journey the impact names: use `cas-qa-craft`
  before parking; cite its commit-bound bundle or real-build ledger. A
  binary-driven ledger runs the supervisor's exact-tip candidate, never a
  compile of your own (cas-qa-craft's evidence-ledger names the sources). Terminal
  paths also need `cas-cli-craft`'s `terminal-qa: PASS` receipt. Docs/test/CI-only
  deliveries do not trigger visual QA.
- Human-facing output (a user-facing label, a demo, or a deliverable a person
  reads): put the `design:` line from
  [DESIGN.md first](../../design-spec/references/design-md-first.md#5-record-it)
  in the close reason.
- Risk: read task `risk` and `proof_targets`. Name the supervisor's
  `ASSEMBLY_PROOF` for full Rust execution. Non-Rust platform risk needs a
  `platform_proof` note with the actual platform, command and passing result.
- Spike: record the decision and evidence; `search_manifest` can record commands
  and hit counts for conclusions based on searches.
- Epic: every child's delivered work must reach the parent before epic close.

### Cross-machine proof logs

Publish and attach a peer proof in one call:

```text
artifact action=publish task_id=<task-id> path=<log> note_type=platform_proof notes="<platform, command, revision, passing result>"
```

Use `note_type=loaded_proof` for a loaded receipt. The note is attached only
when Cloud commits the log; it carries the artifact id, Cloud id and digest.
On the owning machine, run `artifact action=fetch task_id=<task-id> id=<art-id>`
(or `cas artifact fetch --task <task-id> <art-id>`) and read the returned path.
Fetch checks size and SHA-256 before saving. Cite the artifact id in review;
a machine-local path warning means the evidence still needs publishing.
An existing committed id in `task action=notes notes="<receipt> <art-id>"`
also carries its portable reference. Keep the receipt fields in the note.

### Loaded proof for non-Rust targets

When the close gate requires a `loaded_proof` for `risk=concurrency`, add one
`task action=notes note_type=loaded_proof` body with all four fields on one line:

```text
LOADED_PROOF: target=explicit non-Rust (vitest src/example.test.ts + jest tests/example.test.js) parallelism=16 loops=3 result=PASS
```

Replace the example target with the complete target actually exercised. Use
`target=whole`, `target=entire` or `target=full` for a complete Rust target (an
optional `target` word may follow). Use `target=explicit non-Rust (...)` for
other ecosystems. Direct `target=vitest`, `target=jest`, `target=pytest` and
`target=go test` runner forms also work. A Rust module/test-filter `target=`
value does not establish whole-target scope. Legacy `non-Rust target: ...`
and `whole/entire/full target` prose still works.
Record 16 actual concurrent processes/workers with `parallelism=16` or
`concurrency=16`. `-j16` and `--jobs 16` also work when they describe the runner
or harness used; the receipt does not require Cargo flags on a non-Rust runner.
Record at least three loops/runs and `result=PASS` (or SUCCESS, exit 0, status 0).
SHA and log fields are optional; include logs and passed/failed counts for review.
Read the refusal's per-note `parsed=[...]` and `missing=[...]` lists and repair
that note; fields split across notes do not combine into a valid receipt.
Factory Rust checks still defer loaded execution to supervisor assembly under
[discipline.md](discipline.md); this grammar adds no worker check obligation.

## cas-src surface checklist

Record applicable evidence and the reason for each inapplicable surface:

| Changed surface | Delivery evidence |
| --- | --- |
| Builtin skill/agent | One canonical body registered in Claude, Codex and Grok catalogs; regenerate reference history. |
| MCP/CLI | Caller/dispatch registration, CLI parity and docs. |
| Hook/gate | Relevant `config_gen` and `.codex/hooks.json` projection. |
| Migration | Bootstrap/reconciliation expectations and `doctor_snapshot`. |
| Behavior/state | Affected contracts/consumers, including reverse transitions. |
| Public web/terminal | Commit-bound QA receipts for the configured journey. |
| User-visible change | Release-note impact and the project's publication rubric. |

Close with `PASS <sha>` or `ISSUES <sha>` plus known non-blocking defects,
criterion evidence, safety facts and deferred proof ownership. Read actual tool
rejections as state; forward verification-required guidance once to the supervisor.

To wait blocked on a pending verification dispatch without waking the
supervisor, set `status=blocked` and record exactly one blocker note:
`Verification required: exact dispatch <dispatch-id> must record a verdict before close.`
A note with any other text or extra lines is a real blocker and wakes the supervisor.

---
metadata:
  managed_by: cas
---

# Close Gate — Delivery

Read at delivery. Task start lists what close will check; a refused park lists
every failed gate and what it parsed. Fix them all silently, then retry once.

## Commit and push

Commit scoped work. Push at park, at checkpoint or stand-down, and once per
review round; never otherwise (`delivery_mode=local_merge` never pushes).
<!-- cas-src:begin -->
The pre-push hook runs `scripts/fast-rows-pre-push.sh`
when present.
<!-- cas-src:else -->
A configured pre-push hook gives early feedback; CI owns that proof.
<!-- cas-src:end -->
Park reads exact-tip test receipts and never runs checks.

## Rule-driven sites

For an existing stakeholder rule cited in the task or newest supervisor brief, put a
`cas-rule: <id> scope: <JSON-quoted verbatim scope>` comment beside each causal
code/test site, and declare the sites in the brief or close reason as one JSON line:

```text
Rule-driven sites: [{"rule_id":"<id>","path":"src/file.rs","anchor":"<exact trimmed source line>"}]
```

Use `stakeholder.scope` verbatim and check `cas rules refs <id> --json`. A
process-rule citation needs no marker.

## Clean-tree receipt

Run `git status --porcelain` and `git rev-parse HEAD` at every depth: empty
porcelain and HEAD at the delivered SHA. A rejected write may still have changed
disk; escalate unexplained writes instead of closing over them.

For `depth=light`, deliver the minimal working outcome with honest evidence.
Otherwise map every criterion to executed proof or a named proof owner using
`verify-before-claim`, trace affected consumers and registrations, and consume
the receipts from [discipline.md](discipline.md). Every changed Rust crate needs a capped test receipt at the delivered tip; a runner denial is a blocker, not a deferral.

## Ledger preflight

When task start's close checks list a Ledger or `Adversarial probe` line, run
`cas qa ledger init --task <id>`; it prints the path and keeps existing evidence.
Fill it from the real walk, then fix every `cas qa ledger check <path> --task <id>`
diagnostic. A structural PASS does not judge captures.

## Merge and freshness

A successful park reads `PARKED <sha> on <branch>; awaiting merge verdict; nothing more to do`.
Your close parks the delivery; the merge verdict closes it. Never re-close, spawn a verifier or ask anyone to close for you.
The park relay is the completion signal; send merge_request=true only for local_merge.
A dispatched verifier reviews the candidate before publication; PASS publishes
and closes, FAIL reopens it to you with findings. In qa.mode=batch, Cassy closes merged deliveries when the release batch_receipt lands; the worker does nothing.
MERGE REQUIRED is a genuine refusal, such as unpushed or unattributable work:

1. Drain `coordination action=inbox_poll` to `No unread messages`; act on
   unread supervisor messages and re-read the task before corrective edits.
2. Capture `git rev-parse factory/<name>` (or your per-task branch) and check
   `git merge-base --is-ancestor <delivered-tip> <target-tip>`; if it already
   landed, await the verdict rather than amend stale state.
3. For `delivery_mode=local_merge`, keep the local commit and send
   `merge_request=true` with branch and SHA; push only with explicit supervisor
   authorization. Otherwise push and retry close once. While a merge is pending,
   follow [recovery.md](recovery.md#while-the-merge-is-pending).
4. Never open or merge a PR yourself (`gh pr create`); the supervisor owns the merge venue.

`commit_receipt=<sha>` names this task's delivered commit when squash or
cherry-pick changed its identity; `completion_receipt=<json>` delivers an
unpublished tip for candidate verification. A following task starts its own
branch from the epic tip (`factory/<name>-<task-id>`), preserving the parked
one. Never set `status=closed` or invent a verification record to bypass close.

## Upkeep refusals at park

At park, close checks that the delivery kept the journey registry, `.claude/CODEMAP.md`
and `docs/PRODUCT_OVERVIEW.md` true for the paths it changed. Fix a refusal in
this branch. When the change truly needs no upkeep, record the gate's token from
the [escape vocabulary](../../cas-qa-craft/references/journeys.md#escape-vocabulary)
as a decision note with a specific reason; reviewers see each escape. Never
hand-write the `UPKEEP ESCAPE` or `UPKEEP OBSERVE` notes.

## Changelog at park

<!-- cas-src:begin -->
A delivery that changes shipped Rust, `hub-web/src`, user-run scripts or builtin
skills
<!-- cas-src:else -->
A delivery that changes shipped product code or user-run scripts
<!-- cas-src:end -->
adds a titled Unreleased entry under a category and product area (for
example `### Fixed — Commander` with `- **Title** — description`), as its own
`changelog.d/<task-id>.md` where that directory exists, leaving `CHANGELOG.md`
alone. Test-only, docs-only or internal QA deliveries record a decision note
`no changelog: <reason>`.

## Conditional evidence

- In qa.mode=batch (the default) the release's batch QA round at the frozen tip is the only walk; per-task QA evidence applies only in qa.mode=per_task.
- In qa.mode=per_task only: demo, configured user-facing path or a journey the impact names: use `cas-qa-craft`
  before parking and cite its commit-bound bundle or real-build ledger, run on an
  exact-tip candidate from `factory action=candidate_request`. Terminal paths
  also need `cas-cli-craft`'s `terminal-qa: PASS`. Docs/test/CI-only deliveries
  need no visual QA.
- Human-facing output: put the `design:` line from
  [DESIGN.md first](../../design-spec/references/design-md-first.md#5-record-it)
  in the close reason.
- Risk: read task `risk` and `proof_targets`. Full-suite evidence follows
  the project's integration policy. Non-Rust platform risk needs a
  `platform_proof` note with the actual platform, command and passing result.
  A `risk=concurrency` non-Rust target needs one `note_type=loaded_proof` line;
  its refusal prints the parsed and missing fields and an example. Rust
  targets need no loaded proof.
- Spike: record the decision and evidence.
- Epic: every child's delivered work must reach the parent before epic close.

### Cross-machine proof logs

Publish and attach a peer proof in one call:

```text
artifact action=publish task_id=<task-id> path=<log> note_type=platform_proof notes="<platform, command, revision, passing result>"
```

Use `note_type=loaded_proof` for a loaded receipt. The owning machine reads it
with `artifact action=fetch task_id=<task-id> id=<art-id>`. Cite the artifact
id; a machine-local path warning means the evidence still needs publishing.

## cas-src surface checklist

Record evidence for each changed surface:

| Changed surface | Delivery evidence |
| --- | --- |
| Builtin skill/agent | One canonical body in every catalog; regenerated reference history. |
| MCP/CLI | Dispatch registration, CLI parity and docs. |
| Hook/gate | `config_gen` and `.codex/hooks.json` projection. |
| Migration | Bootstrap/reconciliation and `doctor_snapshot`. |
| Behavior/state | Affected consumers, including reverse transitions. |
| Public web/terminal | Commit-bound QA receipts for the configured journey. |
| User-visible change | Release-note impact. |

## Close

Close with `PASS <sha>` or `ISSUES <sha>` plus known non-blocking defects,
criterion evidence, the plan section and its metric receipt when the brief links one,
and safety facts. A missed plan metric is never non-blocking.

When close returns VERIFICATION REQUIRED naming a dispatch, that dispatch's
verdict closes the task; follow [Verification-required close](recovery.md#verification-required-close).

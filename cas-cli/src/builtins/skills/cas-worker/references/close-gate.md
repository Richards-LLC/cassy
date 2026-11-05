---
metadata:
  managed_by: cas
---

# Close Gate — Delivery

Read at delivery. Task start lists what close will check; a refused park lists
every failed gate and what it parsed. Fix them all, then retry once.

## Commit and push

Commit scoped work; push once before parking and once per review round, unless
`delivery_mode=local_merge`.
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
`verify-before-claim`, trace affected consumers and registrations, and name
unexecuted obligations and their assembly owner.

## Ledger preflight

`cas qa ledger init --task <id>` prints the path and keeps existing evidence.
Fill it from the real walk, then fix every `cas qa ledger check <path> --task <id>`
diagnostic. A structural PASS does not judge captures.

## Merge and freshness

Close requires the delivered work on the named target. On MERGE REQUIRED:

1. Drain `coordination action=inbox_poll` to `No unread messages`; act on
   unread supervisor messages and re-read the task before corrective edits.
2. Capture `git rev-parse factory/<name>` (or your per-task branch) and check
   `git merge-base --is-ancestor <delivered-tip> <target-tip>`; if it already
   landed, await the verdict rather than amend stale state.
3. For `delivery_mode=local_merge`, keep the local commit and request the merge
   with branch and SHA. Otherwise push, then send `merge_request=true` with
   target and SHA. The verifier reviews the candidate before publication; PASS
   publishes and closes, FAIL holds only that delivery. A Tier 1 delivery in
   `qa.mode=batch` waits for the release verdict; you have nothing to do. Do not
   re-close or block on it; follow [recovery.md](recovery.md#while-the-merge-is-pending).
4. For a protected target, follow its PR flow. Ask for an epic merge rather than
   `gh pr create --base epic/...`; the supervisor owns the merge venue.

`commit_receipt=<sha>` names this task's delivered commit when squash or
cherry-pick changed its identity; `completion_receipt=<json>` delivers an
unpublished tip for candidate verification. A following task starts its own
branch from the epic tip (`factory/<name>-<task-id>`), preserving the parked
one. Never set `status=closed` or invent a verification record to bypass close.

## Upkeep refusals at park

Close checks that the delivery kept the journey registry, `.claude/CODEMAP.md`
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

- Demo, configured user-facing path or a journey the impact names: use `cas-qa-craft`
  before parking and cite its commit-bound bundle or real-build ledger, run on an
  exact-tip candidate from `factory action=candidate_request`. Terminal paths
  also need `cas-cli-craft`'s `terminal-qa: PASS`. Docs/test/CI-only deliveries
  need no visual QA.
- Human-facing output: put the `design:` line from
  [DESIGN.md first](../../design-spec/references/design-md-first.md#5-record-it)
  in the close reason.
- Risk: read task `risk` and `proof_targets`. Name the supervisor's
  `ASSEMBLY_PROOF` for full Rust execution. Non-Rust platform risk needs a
  `platform_proof` note with the actual platform, command and passing result.
  A `risk=concurrency` non-Rust target needs one `note_type=loaded_proof` line;
  its refusal prints the parsed and missing fields and an example. Worker Rust
  checks still defer loaded execution to assembly.
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
criterion evidence, the linked plan section and its metric receipt, safety facts
and deferred proof ownership. A missed plan metric is never non-blocking.

Workers close their own tasks. When close returns VERIFICATION REQUIRED naming a
dispatch, forward it once to the supervisor with the delivery SHA, set
`status=blocked`, record exactly this one-line blocker note, wait, and retry
close after the verdict:
`Verification required: exact dispatch <dispatch-id> must record a verdict before close.`
A note with other text or extra lines is a real blocker and wakes the supervisor.

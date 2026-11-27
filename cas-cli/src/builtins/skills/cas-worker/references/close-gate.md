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
Green lane CI on the exact tip plus named tests is the proof; missing local receipts warn. Park never runs checks.

## Rule-driven sites

For an existing stakeholder rule cited in the task or newest supervisor brief, put a
`cas-rule: <id> scope: <JSON-quoted verbatim scope>` comment beside each causal
code/test site, and declare the sites in the brief or close reason as one JSON line:

```text
Rule-driven sites: [{"rule_id":"<id>","path":"src/file.rs","anchor":"<exact trimmed source line>"}]
```

Use `stakeholder.scope` verbatim and check `cas rules refs <id> --json`. A
process-rule citation needs no marker.

## Pre-park pass

Run these on your delivery diff only, never the whole repository. The principles are paraphrased from pstack (Lauren Tan, MIT).

- **Comments:** run [cas-no-comments](../../cas-no-comments/SKILL.md). Its output is deletions or a reshape flag; you never add a comment.
- **Subtract before you add:** remove any branch no caller reaches and any struct that duplicates an existing one; a smaller diff that fully works wins.
- **Same-surface proof:** a bug fix exercises the incident's real entry point; a failing-first run is optional.

## Clean-tree receipt

Run `git status --porcelain` and `git rev-parse HEAD` at every depth: empty
porcelain and HEAD at the delivered SHA. A rejected write may still have changed
disk; escalate unexplained writes instead of closing over them.

For `depth=light`, deliver the minimal working outcome with honest evidence.
Otherwise map every criterion to executed proof or a named proof owner using
`verify-before-claim`, trace affected consumers and registrations, and consume
the receipts from [discipline.md](discipline.md).

## Ledger preflight

When task start's close checks list a Ledger or `Adversarial probe` line, run
`cas qa ledger init --task <id>`; it prints the path and keeps existing evidence.
Fill it from the real walk, then fix every `cas qa ledger check <path> --task <id>`
diagnostic. A structural PASS does not judge captures.

## Report deliverables

A report under `docs/` (an `.html` beside its same-basename `.md`) parks only as a
triplet: the `.md`, the `.brief.md` (with `Audience:` and a `## Critique` table meeting the
cas-ui-craft floor) and the `.html`. The HTML must declare the DESIGN.md tokens, use no network,
put its `data-verdict` first and stay within its height budget. Business reports carry no
task ids and no jargon. A visual-QA PASS is necessary, not sufficient. The G7 refusal lists each gap;
see [cas-html-reports](../../cas-html-reports/SKILL.md).

## Merge and freshness

A successful park reads `PARKED <sha> on <branch>; awaiting merge verdict; nothing more to do`.
A dispatched verifier reviews the candidate before publication; PASS publishes
and closes, FAIL reopens it to you with findings. A MERGE REQUIRED refusal
follows [recovery.md](recovery.md#merge-required); while a merge is pending,
follow [recovery.md](recovery.md#while-the-merge-is-pending).

`commit_receipt=<sha>` names this task's delivered commit when squash or
cherry-pick changed its identity; `completion_receipt=<json>` delivers an
unpublished tip for candidate verification. A following task starts its own
branch from the epic tip (`factory/<name>-<task-id>`), preserving the parked
one.

## Shared docs

Never edit `.claude/CODEMAP.md` or `docs/PRODUCT_OVERVIEW.md`; release prep
regenerates them. Add a bug case as its own `docs/qa/bug-families/<slug>.yaml`
and, where `docs/product-overview.d/` exists, product changes as
`docs/product-overview.d/<task-id>.md`. Missing overview fragments (G-167) or bug families (G-168) warn.
Release prep generates shared docs and reports missing upkeep.

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

The one independent review looks at screenshots of changed screens (phone/desktop,
light/dark). Missing screenshots warn.

## Conditional evidence

- [Batch QA](details.md#batch-qa) (the default) owns the walk; delivery evidence is advisory at close. Fixture and integration receipts
  prove changed behavior; missing live walks and batch receipts warn.
  Independent review owns available screenshots.
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
VERIFICATION REQUIRED follows [Verification-required close](recovery.md#verification-required-close).

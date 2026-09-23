# QA evidence bundle at close (design, cas-0cd5)

Status: draft for supervisor approval, 2026-09-23.

## Problem

Customers keep receiving deliveries whose bugs anyone would spot by opening
the product. The `cas-qa-craft` skill asks the implementer to run the
product and keep evidence, but nothing enforces it. Code facts:

- `close_ops.rs` never reads `demo_statement`.
- On the push-branch path, the worker's first close goes straight to the
  factory merge gate (`close_ops.rs:4752`) and is parked as
  `AwaitingMerge` (`park_task_awaiting_merge`, `close_ops.rs:3501`). No
  earlier check asks for evidence. The pre-park checks (blockers, override,
  receipts, tmpfs citation, pending dispatch, halt, repo resolution; lines
  3990–4741) are all about identity and state.
- The only evidence-bearing close checks are the risk proofs
  (`validate_risk_close_proofs_with_base_and_target_and_cache`, called at
  `close_ops.rs:6235`). They run on the post-merge re-close, after the
  task-verifier's verdict. By then the supervisor has merged and cas-619f's
  independent QA pass has been dispatched.

So an implementer can park, and get an independent reviewer spawned, for a
user-facing change they never ran.

## Decision summary

| Question | Decision |
| --- | --- |
| What is required | The cas-c3b8 bundle, contract v1 (`~/.cas/artifacts/cas-c3b8/bundle-contract.md`). This is a `bundle.json` manifest plus its files, under `<artifacts_root>/<task-id>/`, cited by a `platform_proof` note `qa-bundle: <abs>/bundle.json`. |
| Who must produce it | The implementer, for a delivery that is **web user-facing** (see Eligibility). |
| Where it is enforced | **Before the merge gate** in `cas_task_close_with_completion`, after repo and branch resolution (`~4745`). It runs on every close attempt until the task closes, so the first close cannot park and the re-close cannot finish without a valid bundle for the delivered head. |
| Staleness | Every listed file's mtime and `created_at` must be later than the delivered head's committer time. `head_sha` must equal the delivered head, or be a descendant of it. |
| Trace proof | Parsed natively from `trace.zip`'s `test.trace`. It needs ≥1 passing `Expect "` step and no failed `Expect "` step. `trace-actions.txt` must list at least one `Expect "` line. No `npx` runs in the close path. |
| Skip markers | Any delivery that **adds** `test.fixme`, `test.skip` or `.only` markers to a JS/TS test file is rejected, whether or not it is user-facing. A marker is allowed when its line or the line above carries `cas-allow-skip: <reason>`. This is the healer finding from cas-d7b7. |
| Bypass | `supervisor_override=true` with a non-empty reason from a live supervisor (the existing mechanism). It is logged as a `✅ DECISION` note that names the gate. |
| Config | `qa.evidence_gate` (bool, default `true`) in cas-619f's `[qa]` section. |

## 1. Eligibility (shared with cas-619f)

The gate uses the same predicate as the independent QA pass, so a delivery
is either user-facing for both gates or for neither. cas-619f's
`qa_pass::delivery_eligibility` currently returns early on
`qa.independent_pass`. I will split that function: a flag-free
`user_facing_reasons(task, qa, changed_paths, journeys)`, which both gates
call, and the two thin wrappers `delivery_eligibility` (the independent pass)
and `evidence_eligibility` (this gate). I'm coordinating this with
zealous-cheetah-52.

Shared rules, unchanged from cas-619f:

- **Never gated:** epics, `qa-pass` work items, labels alone, and a known
  diff made only of docs, tests, fixtures or CI (`is_non_surface_path`).
- **User-facing:** a surface path that touches a catalog journey
  (`scripts/journeys-for-diff.py --paths`), a surface path matching
  `qa.user_facing_paths`, or a `demo_statement`.

This gate adds one refinement, because the bundle is a Playwright artifact:

| Reason the delivery is user-facing | Evidence required |
| --- | --- |
| Journey, or `user_facing_paths` match (a web surface) | The cas-c3b8 bundle (§2) |
| `demo_statement` only, with no web surface in the diff (for example a CLI change) | `<task>/LEDGER.md`: non-empty, fresher than the delivered head, with ≥1 `PASS` row. The contract says CLI-only cells produce no Playwright bundle (§2 of the contract). |

Where the diff comes from:

- **First close** (not yet merged): `merge-base(parent, factory/<worker>)..factory/<worker>`.
- **Re-close:** cas-619f's `integrated_paths(repo, head, target)`, meaning the
  first merge on the ancestry path.
- When neither can be computed, the gate falls back to `demo_statement`
  only. That is conservative: it can demand a ledger, but never a bundle for
  an unknown diff.

The delivered head is `task.deliverables.factory_branch_anchor` when a
park recorded one. Otherwise it is the resolved `factory/<assignee>` tip,
or the commit receipt for a standalone task.

## 2. Bundle validation

The gate finds the citation first. The newest task note containing
`qa-bundle: <path>` gives the path. That path must canonicalise inside
`<artifacts_root>/<task-id>/`; a symlink escape fails, using the
`artifacts::paths` resolution. It must not be under `independent-qa/`,
because a reviewer's bundle is not the implementer's evidence. The check
does not look for a `platform_proof` token, because the existing
risk=platform check owns that token.

Checks, in order. Each rejection names the failing key and the command
that produces it:

1. `bundle.json` parses, `schema == 1`, `task_id` equals the task, and
   `producer` is `cas-qa-craft` or `journey`.
2. Every key the contract marks "always" is present. Each listed file exists
   inside the bundle directory, is a regular file, and is non-empty.
   `cells` and `polish_screenshots` each have ≥1 entry, and
   `polish_screenshots` has exactly the four
   `{light,dark}-{desktop,phone}` renders.
3. **Staleness.** `created_at` and every file's mtime are later than the
   delivered head's committer time. `head_sha` is 40 hex characters,
   resolves in the repo, and is the delivered head or a descendant of it.
   If a commit lands after the bundle was made, the bundle goes stale.
4. **Trace.** `trace.zip` opens. Its `test.trace` has ≥1 `before` event
   titled `Expect "…"` whose matching `after` event has no `error`, and no
   `Expect` step whose `after` event has an `error`. `trace-actions.txt`
   contains an `Expect "` line.
5. **Polish.** The first line of `visual-qa.md` contains `PASS`, the last
   non-empty line of `visual-qa.stdout` is `PASS`, and
   `visual_qa_status == "pass"`. `"unavailable"` rejects unless the
   supervisor overrides.
6. **Critique.** `critique_score` has the five dimensions, each 0–5. It must
   meet the floor: distinctiveness, fit and hierarchy each ≥ 4, and no
   dimension at 0.
7. **Accessibility.** When `visual_change` is true, `a11y` lists the three
   files: forced-colors, reduced-motion and contrast-more.

The gate reads only the keys the manifest lists and never rejects a
bundle for extra files (contract §1).

The rejection shape matches the existing risk-proof messages:

```text
TASK CLOSE REJECTED: <task> is user-facing (<reasons>) and its QA evidence bundle is <problem>.
Produce it with: <exact command>. Then add `task action=notes id=<task> note_type=platform_proof notes="qa-bundle: <artifacts_root>/<task>/qa/bundle.json"` and retry close.
Contract: cas-qa-craft references/evidence-bundle.md.
```

Examples of the exact commands:

- Missing trace actions:
  `cd <bundle> && npx playwright trace open trace.zip && npx playwright trace actions > trace-actions.txt; npx playwright trace close`
- Visual QA:
  `node scripts/visual-qa.mjs --strict --artifact-dir <bundle>/visual-qa <url> > <bundle>/visual-qa.stdout 2>&1`
- Stale bundle: names the commit that post-dates it and says to re-run the
  worked example against `<head>`.

## 3. Skip and fixme markers

cas-d7b7 found that Playwright's healer answers a real regression with
`test.fixme()`. The run then exits 0 and stays green after the bug is
fixed. The gate therefore scans the **added** lines of the delivery diff
(`git diff -U0 <range>`), restricted to JS/TS test files: `*.spec.*`,
`*.test.*`, and files under `e2e/`, `tests/` or `test/`. It looks for:

`test.fixme(`, `test.skip(`, `test.describe.fixme(`, `test.describe.skip(`,
`test.only(`, `test.describe.only(`, `it.skip(`, `it.only(`, `describe.skip(`,
`describe.only(`, `xit(`, `xdescribe(`

A match rejects the close and lists each `file:line`, unless the matched
line or the line above it carries `cas-allow-skip: <non-empty reason>`
(for example a real `test.skip(browserName === "webkit", …)`). Allowed
markers are copied into a `✅ DECISION` note at close.

This check applies to **every** delivery, not only user-facing ones. A
healer-only delivery is test-only, so eligibility alone would never see it.

## 4. Relation to cas-619f (no double-blocking)

| | cas-0cd5 (this) | cas-619f |
| --- | --- | --- |
| Question | Did the implementer run it? | Did someone else confirm it? |
| Moment | Every close, before the merge gate | Dispatch at park, gates at merge and re-close |
| Evidence dir | `<task>/qa/` (or `<task>/journeys/<id>/`) | `<task>/independent-qa/round-<n>/` |
| Eligibility | The shared `user_facing_reasons` | The shared `user_facing_reasons` |
| Waiver | `supervisor_override` on close, logged | `qa_waive`, logged |

- The two gates ask different questions at different moments, so they never
  reject the same missing thing twice. A failing evidence gate prevents the
  park, so no independent reviewer is spawned for a delivery the implementer
  never ran. That saves a taste-lane worker.
- On re-close, a bundle already validated at park still validates, because
  the delivered head is unchanged. The evidence gate adds no new
  requirement after the merge unless the implementer committed again.
- A `qa-pass` work item is never eligible for either gate.

## 5. Placement and code shape

- A new module, `cas-cli/src/qa_evidence.rs`, holds only the validation: the
  bundle, the ledger, the trace parser and the skip scanner. It takes paths,
  the delivered head and a git runner, so it is unit-testable with temp dirs
  and a scratch repo.
- `close_ops.rs` makes one call, `validate_qa_evidence_close(...)`,
  immediately before `run_factory_branch_merge_gate_with_attribution`, when
  `close_disposition` is Delivered and the task is not an epic or no-code.
  Standalone (non-factory) tasks take the same path; there is simply no
  park after it.
- Trace parsing uses the `zip` crate, already in `Cargo.lock` at 2.4.2
  through the updater, with the same features, so no new download is
  needed.
- **Dependency:** this builds on cas-619f's `qa_pass.rs` and `QaConfig`
  (`factory/zealous-cheetah-52`, not yet on the epic). I'll merge their
  branch into mine once their tip compiles, or rebase when the supervisor
  lands it.

## 6. Tests (proof targets: cas task close gate tests, cas verification tests)

**Unit tests** (`qa_evidence`):

- missing `bundle.json`
- valid bundle
- each file key missing or empty
- stale by mtime, by `created_at`, and by `head_sha` being an ancestor
- a failed `Expect` in the trace
- no `Expect` step at all
- critique below the floor
- `visual_qa_status: unavailable`
- `visual_change` without a11y
- a citation outside the task dir, or a symlink escape
- an `independent-qa/` citation
- skip markers: rejected, allowed with a reason, ignored in non-test files
- ledger-tier pass and fail

**MCP close tests** (`mcp_tools_test`, task close module):

- A demo_statement web task with no bundle is rejected with an actionable
  message.
- The same task with a valid bundle passes through to MERGE REQUIRED
  (park).
- A commit made after the bundle rejects as stale.
- A docs-only diff with a demo_statement is not gated.
- An added `test.fixme` rejects even on a test-only delivery.
- `supervisor_override` with a reason passes and logs a decision note.

## 7. Docs and skills

- **cas-qa-craft `SKILL.md`** (3 mirrors): a short "Close gate" pointer. The
  body lives in the cas-c3b8 reference `references/evidence-bundle.md`, so
  the 120-line cap holds.
- **cas-worker:** one line saying user-facing closes need the `qa-bundle:`
  note, and new skip markers need `cas-allow-skip:`.
- **cas-supervisor:** how to read the rejection and when an override is
  legitimate.

## Open questions for the supervisor

1. Is the ledger tier (demo_statement with no web surface) acceptable, or
   should demo-only CLI tasks be ungated?
2. Should the skip-marker check apply to every delivery (as proposed), or
   only to user-facing ones?

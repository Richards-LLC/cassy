---
name: cas-qa-craft
description: Use when a factory delivery needs QA evidence before close — a non-empty demo_statement, a changed user-facing path, or a touched user journey — proven against the real build.
metadata:
  managed_by: cas
---

# Delivery QA evidence

Turn a delivery's QA trigger into a capped exploration matrix and prove it
against the named build. The close gate fires on any of three triggers: a
non-empty `demo_statement`, a diff that touches a `qa.user_facing_paths` glob,
or a touched journey in `docs/qa/journeys.md`. This is an evidence pass, not a fixture test
and not a substitute for unit or integration tests. Time-box the whole pass to
**30 minutes**; an honest incomplete ledger beats a late, invented result.
For an epic with child demos, use the supervisor's
[epic flow walk](../cas-supervisor/references/epic-flow-walk.md):
one combined matrix with a **60-minute** box overrides the task defaults below.
If you started a `qa-pass` task, you are the independent reviewer of someone
else's delivery: follow [references/independent-pass.md](references/independent-pass.md)
instead of the procedure below.

## Procedure

1. Read the active task with `task action=show`; name the binary version or
   commit SHA and write a one-line scope sentence before exercising anything.
   Check the three triggers the close gate uses. If the `demo_statement` is
   empty and no child has one, and no user-facing path or journey is touched,
   stop: do not invent a matrix. With no demo, row one comes from the touched
   path or journey.
2. **Telemetry sweep, first.** Read `[qa] telemetry_sweep`. If it is set, run
   it read-only from the project root as described in
   [references/telemetry-sweep.md](references/telemetry-sweep.md), write
   `sweep: configured — <path>` in the ledger header, and add each valid
   finding as a `telemetry sweep` row labeled `eyewitness/telemetry`. If it is
   unset, write the exact header line `sweep: not configured`.
3. Build the exploration matrix with [references/matrix-builder.md](references/matrix-builder.md):
   derive the first row from the demo, then add **at least three unmentioned
   conditions**, at least one adjacent surface, and no replay cells after row
   one. Include empty, failure/timeout, revisit, resize/phone, or keyboard
   conditions as risk warrants. Cap the matrix at **8 cells**. Write each
   expected result in the user's words before running its cell. When the
   change touches a user journey, walk it from the real entry point to the
   user's goal and score the experience, not just pass/fail
   ([references/journeys.md](references/journeys.md)).
4. Write the ledger to `~/.cas/artifacts/<task-id>/LEDGER.md` using
   [references/evidence-ledger.md](references/evidence-ledger.md). Drive every
   cell against the real build: Playwright using project/`cas-playwright-debug`
   conventions for web or hub surfaces, or the real binary for CLI. Register
   long-lived servers through `cas-servers`; never substitute fixtures.
5. Capture one screenshot or terminal capture per cell, and label every row
   `source-inferred`, `fixture`, `real-build`, or `eyewitness`. A label weaker
   than the cell needs is `NOT EXERCISED`, never `PASS`; never write “partial”.
   When the 30-minute box expires, mark every unrun cell `NOT EXERCISED`.
   For web or hub cells, write the **evidence bundle** to
   `~/.cas/artifacts/<task-id>/qa/` with
   [references/evidence-bundle.md](references/evidence-bundle.md). It holds:
   - a trace recorded with `snapshots: { dom: true, aria: true, screen: true }`
   - a `page.screencast` receipt with `showActions` and a `showChapter` for
     each cell
   - a `toMatchAriaSnapshot`-asserted final state
   - `forcedColors`/`reducedMotion`/`contrast` captures when the change is
     visual
   - polish evidence: desktop and phone renders in light and dark,
     `<skills-dir>/cas-ui-craft/scripts/visual-qa.mjs --strict` output, and a cas-ui-craft critique
     score
   Cite its `bundle.json` in a `platform_proof` note and in the close reason.
6. Grep the touched feature for `MIN_`, `MAX_`, `_MINUTES`, `_MS`, `_SECS`,
   `THRESHOLD`, `GRACE`, `DEBOUNCE`, and `RETRY`; record whether each constant is
   predictable from the user's visible contract. For terminal states, dump all
   visible text across surfaces and flag contradictory claims.
7. Record one task per defect found; do not patch from this QA pass. Add the
   ledger path, build revision, label split, and verdict counts to a task note
   and the `task action=close` reason. Stop registered servers before close.

## Close gate

Close enforces this evidence before a user-facing delivery can park or close
(`qa.evidence_gate`). A web-surface delivery needs the evidence bundle
(`references/evidence-bundle.md`): `<artifacts>/<task-id>/qa/bundle.json` for
the delivered commit, cited with `task action=notes note_type=platform_proof
notes="qa-bundle: <abs path>/bundle.json"`. It must be newer than your last
commit, record at least one passing `Expect`, and pass visual QA and the
critique floor, including for a journey bundle. A demo-only change with no web
surface needs a fresh `LEDGER.md` with a `PASS` / `real-build` row, plus a
cas-cli-craft `terminal-qa: PASS` report under `<task-id>/terminal-qa/` when the
diff touches `qa.terminal_render_paths`
(`node <skills-dir>/cas-cli-craft/scripts/terminal-qa.mjs --label <cmd> --out <dir> -- <cmd>`).
`<skills-dir>` is the harness skill directory: `.claude/skills`, `.codex/skills`
or `.grok/skills`. Any delivery that adds `test.fixme`, `.skip` or `.only` is
refused unless the marker or the line above it carries `cas-allow-skip: <reason>`.
Rejections name the exact command that produces what is missing.

## Boundaries

Keep this skill focused on user-flow evidence. The worked matrix is in
[references/exemplar.md](references/exemplar.md); `cas-playwright-debug` owns
framework-specific diagnosis, `cas-servers` owns process lifecycle, and the
task verifier owns judgment. Add automation-caused artifacts to the ledger's
honesty section. Do not modify the verifier to make a missing or failing
capture pass.

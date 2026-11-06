---
name: cas-qa-craft
description: Use when a factory delivery needs QA evidence before close — a non-empty demo_statement, a changed user-facing path, or a touched user journey — proven against the real build.
metadata:
  managed_by: cas
---

# Delivery QA evidence

In qa.mode=batch (the default) the release's batch QA round at the frozen tip is the only walk; per-task QA evidence applies only in qa.mode=per_task.
Start with the [batch flow](references/independent-pass.md#batch-default).
Prove the named build's demo, `qa.user_facing_paths`, `qa.api_paths` and impacted journeys.
API globs declare non-rendered server code, including server-only TSX; rendered paths need visual evidence.
This evidence pass does not replace fixture, unit or integration tests. Time-box every pass, batch
round included, to `qa.pass_timeout_mins`; an honest incomplete ledger beats a late, invented result.
For an epic with child demos, the batch round walks one combined [epic matrix](../cas-supervisor/references/epic-flow-walk.md). On a `qa-pass` task you review someone else's delivery: use the API path
below for API-only reasons; otherwise use [the independent pass](references/independent-pass.md).
Record its verdict with `verification action=qa_record`, naming the round's `LEDGER.md`.

## Procedure

1. Read the active task with `task action=show`; name the binary version or commit SHA ([map the
   build](#walk-rules) for a served app) and write a one-line scope sentence before exercising
   anything. Check the triggers above. If no demo, API/user-facing path or journey applies, stop: do
   not invent a matrix. With no demo, row one comes from the touched path or journey.
2. **Telemetry sweep, first.** Read `[qa] telemetry_sweep`. If it is set, run it read-only from the
   project root as described in [references/telemetry-sweep.md](references/telemetry-sweep.md),
   write `sweep: configured — <path>` in the ledger header, and add each valid finding as a
   `telemetry sweep` row labeled `eyewitness/telemetry`. If it is unset, write the exact header line
   `sweep: not configured`.
3. For API cells follow [Fewest tests](../cas-tdd/SKILL.md#fewest-tests): one capture per changed
   status/error code, including refusals. Otherwise use
   [references/matrix-builder.md](references/matrix-builder.md): demo first,
   **at least three unmentioned conditions**, one adjacent surface, no replay cells. Cap the matrix at **8 cells**;
   state expectations before execution. The [walk rules](#walk-rules) choose and order the first
   rows. Each live journey in the QA task's journey table (or `cas qa impact --task <id>`) gets one
   row, walked from entry to goal with its `evidence_required` read-back, under the journey id and a
   verdict `qa_record` accepts ([journey rows](references/journeys.md#journey-rows)); score the
   experience. Upkeep refusals and their escape tokens are in the
   [escape vocabulary](references/journeys.md#escape-vocabulary). Multi-surface walks use `runSurfaceLanes` from the project's pinned kit: bounded parallel jobs, separate ledgers,
   browser profiles and personas per channel/viewport, never a serial 1440→390→SMS replay ([kit](references/verify-harness.md), [maintenance](references/maintain.md)).
4. Run `cas qa ledger init --task <id>` and edit the printed project-scoped
   ledger ([evidence guidance](references/evidence-ledger.md)). Drive every cell against the real
   build: Playwright CLI by default (MCP for long exploratory walks); use project/`cas-playwright-debug` 1.64 recording and healer guards for rendered web/hub surfaces,
   or the real binary for CLI. For API cells, `LEDGER.md` is the HTTP request/status/body transcript
   against the real server build (for example `next build` then `next start`) or a configured
   deployed sandbox with project-provided credentials. Pair it with `contracts.diff` against
   vendored fixtures; these are the only two required API artifacts ([API
   format](references/evidence-bundle.md#api-surfaces)). Register long-lived servers through
   `cas-servers`; never substitute fixtures.
   For Cassy binaries, request the exact-tip daemon candidate via
   `factory action=candidate_request task_id=<your-task> revision=<full-sha>`; cite its receipt
   ([sources](references/evidence-ledger.md)).
5. Capture rendered cells with screenshots, CLI cells with terminal captures, and API cells with
   HTTP transcripts. Choose evidence labels by how the result was obtained using
   the scaffold and [evidence guidance](references/evidence-ledger.md). When
   the box expires, mark every unrun cell `NOT EXERCISED: <reason>`. For rendered web/hub cells,
   write the **evidence bundle** to `~/.cas/artifacts/<project-key>/<task-id>/qa/` with
   [references/evidence-bundle.md](references/evidence-bundle.md), including its trace, ARIA and
   polish requirements even when `visual_change: false`. For a visual change, also write its
   before/after with `cas deliverable capture --task <task-id> --target <target> --side both` to `qa/pairs/`;
   cite `files.pairs` and `pairs_status` ([pairs](references/evidence-bundle.md#beforeafter-pairs)).
   If unavailable, omit `files.pairs`; set `pairs_status: "unavailable"` and a one-line `pairs_reason`.
   Deploy-only: run `cas deliverable capture --task <task-id> --side before` at task start, before deploying.
   Reports and release notes read pairs there. Cite `bundle.json` in a `platform_proof` note and the close reason. API-only changes
   use the transcript and diff, with credentials redacted; record sandbox reset and the check that
   no smoke rows remain inline.
6. Grep the touched feature for `MIN_`, `MAX_`, `_MINUTES`, `_MS`, `_SECS`, `THRESHOLD`, `GRACE`,
   `DEBOUNCE`, and `RETRY`; record whether each constant is predictable from the user's visible
   contract. For terminal states, dump all visible text across surfaces and flag
   contradictory claims. Run the [walk rules](#walk-rules) sweep for controls the user can't act on.
7. Run `cas qa ledger check <path> --task <id>` and repair every structural
   diagnostic before submitting evidence. A qa-pass task records one task per defect; implementers fix their own delivery. Add the ledger path, build
   revision, label split, and verdict counts to a task note and the `task action=close` reason. Stop
   registered servers before close.

## Walk rules

These hold for every walk, task pass or [independent pass](references/independent-pass.md).

- **Every entry path to a shared step.** Before a verdict on a step several flows share (signup,
  identity, claim, checkout), list every entry path reaching it from source routes and any journey
  catalog. Walk them in order of real traffic, not convenience: the path that supplies the changed
  input early passes while the rest break. Entry paths take the first rows; any past the cap stay
  `NOT EXERCISED: <reason>`.
- **Grade the visible result first.** Cite a spec `path:line` and quote per expectation;
  uncited expectations are spec questions. Wait for real reply text, record measured reply time, and open delivered artifacts. Storage read-back only corroborates. Use one row per defect across
  widths/channels; run `ifAsked` answers only for the current open question.
- **Corroborate persisted invariants** from the system of record (database query, API read, admin view) after the final step, not from the screen. Grade paid/account routes and adjudicate harness-only reds using [server read-back](references/evidence-ledger.md#server-read-back-and-harness-failures); put the read and result in `observed`.
- **Map the build.** For a served app, record the backend and frontend SHAs served at the start and
  end of each walk; disclose a mixed build in the ledger header. A backend-only merge ships no new
  frontend build: prove it with `git diff --stat <served-frontend-sha> <tip> -- <frontend paths>`.
- **Sweep for controls the user can't act on:** an instruction without a control, an unselectable
  option, a mislabeled control, a dead end, lost intent between steps, an unreachable route, a stale
  overlay. Each is a defect row.
- **Keep fixtures valid.** A single-use token (invite, magic link, one-time code) is spent by the
  first attempt: mint a fresh one rather than retrying in a new browser. Use locale-valid country,
  phone and address fixtures, never reserved or example ranges.

## Close gate

In qa.mode=per_task only: close enforces fresh, commit-bound evidence (`qa.evidence_gate`). Rendered surfaces need
`qa/bundle.json`, cited in a `platform_proof` note as `qa-bundle: <abs path>/bundle.json`; follow
its contract, including passing `Expect`, visual QA, critique floors and base-build comparison for
scoped findings. API-only changes need `LEDGER.md` and `contracts.diff`; demo-only non-web changes
need a fresh `LEDGER.md` with a `PASS` / `real-build` row, plus a cas-cli-craft `terminal-qa: PASS`
report under `<task-id>/terminal-qa/` when the diff touches `qa.terminal_render_paths` (`node
<skills-dir>/cas-cli-craft/scripts/terminal-qa.mjs --label <cmd> --out <dir> -- <cmd>`).
`<skills-dir>` is the harness skill directory: `.claude/skills`, `.codex/skills` or `.grok/skills`.

Any delivery that adds `test.fixme`, `.skip` or `.only` is refused unless the marker or the line
above it carries `cas-allow-skip: <reason>`. Rejections name the exact command that produces what is
missing. Tier 0 (docs, tests, changelog) walks no journeys; Tier 2 (guards, CI, release, close gate) needs a ledger row starting `Adversarial probe:`. Every finding records severity × reach, journey and runtime repro in its triage. Code-read-only findings are theoretical P4 backlog; only money/data loss on a declared main journey gets a P2 exception. Reproduced main-journey FAILs come first. Honor the inherited operator focus.

## Boundaries

Keep user-flow evidence here; see the [worked matrix](references/exemplar.md) and check the delivery's `design:` against
[DESIGN.md first](../design-spec/references/design-md-first.md). `cas-playwright-debug` owns diagnosis, `cas-servers` process lifecycle, the task verifier judgment.
Log automation-caused artifacts in the ledger's honesty section; never modify the verifier to pass a missing capture.

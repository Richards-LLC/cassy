---
name: cas-qa-craft
description: Use when a delivery or a release batch QA round needs evidence from the real build — a non-empty demo_statement, a changed user-facing or API path, or a touched user journey.
metadata:
  managed_by: cas
---

# Delivery QA evidence

In `qa.mode=batch` (the default) the release's batch QA round at the frozen tip is the only walk; per-task evidence applies only in `qa.mode=per_task` or on an explicit QA request.
Start with the [batch flow](references/independent-pass.md#batch-default). For an epic with child demos, the batch round walks one combined [epic matrix](../cas-supervisor/references/epic-flow-walk.md).

Prove the named build's demo, `qa.user_facing_paths`, `qa.api_paths` and impacted journeys.
API globs declare non-rendered server code, including server-only TSX; rendered paths need visual evidence.
This pass does not replace fixture, unit or integration tests. Time-box every pass, batch round included, to `qa.pass_timeout_mins`: an honest incomplete ledger beats a late, invented result.
On a `qa-pass` task you review someone else's delivery: use the API path below for API-only reasons, otherwise [the independent pass](references/independent-pass.md). Record its verdict with `verification action=qa_record`, naming the round's `LEDGER.md`.

## Procedure

1. Read the task with `task action=show`, name the binary version or commit SHA ([map the
   build](#walk-rules) for a served app) and write a one-line scope sentence. If no demo,
   API/user-facing path or journey applies, stop without a matrix. With no demo, row one comes
   from the touched path or journey.
2. **Telemetry sweep, first.** If `qa.telemetry_sweep` is set, run it read-only from the project
   root per [telemetry-sweep.md](references/telemetry-sweep.md), write
   `sweep: configured — <path>` in the ledger header, and add each valid finding as a
   `telemetry sweep` row labelled `eyewitness/telemetry`. Otherwise keep the scaffold's
   `sweep: not configured` header line.
3. Build the matrix. API cells follow [Fewest tests](../cas-writing-tests/SKILL.md#fewest-tests):
   one capture per changed status/error code, refusals included. Other cells follow
   [matrix-builder.md](references/matrix-builder.md); the [walk rules](#walk-rules) choose and order
   the first rows. Each live journey in the QA task's journey table (or `cas qa impact --task <id>`)
   gets one row walked from entry to goal with its `evidence_required` read-back
   ([journey rows](references/journeys.md#journey-rows); refusals and escapes in the
   [escape vocabulary](references/journeys.md#escape-vocabulary)). Score the experience.
   Multi-surface walks use `runSurfaceLanes` from the project's pinned [walk kit](references/verify-harness.md):
   bounded parallel jobs with separate ledgers, browser profiles and personas per channel/viewport,
   not a serial 1440→390→SMS replay ([maintenance](references/maintain.md)).
4. Run `cas qa ledger init --task <id>` and fill the printed project-scoped ledger
   ([evidence guidance](references/evidence-ledger.md)). Drive every cell against the real build
   with the project's locked Playwright and Node. Browser authoring, target/network policy, auth,
   locks and test agents follow [cas-playwright-tests](../cas-playwright-tests/SKILL.md). Use
   Playwright CLI by default (MCP for exploratory walks) and `cas-playwright-debug` for failures.
   - API cells: `LEDGER.md` is the HTTP request/status/body transcript against the real server
     build (for example `next build` then `next start`) or a configured deployed sandbox with
     project-provided credentials, paired with `contracts.diff` against vendored fixtures; these
     are the only two required API artifacts ([API format](references/evidence-bundle.md#api-surfaces)).
   - Register long-lived servers through `cas-servers`; never substitute fixtures.
   - Cassy binaries: request the exact-tip candidate with
     `factory action=candidate_request task_id=<your-task> revision=<full-sha>` and cite its receipt
     ([sources](references/evidence-ledger.md)). Candidate MCP walks use `cas serve --sandbox <dir>`
     ([isolation](references/verify-harness.md#isolated-candidate-serve)).
5. Capture rendered cells as screenshots, CLI cells as terminal captures and API cells as HTTP
   transcripts; label each by how the result was obtained. When the box expires, mark every unrun
   cell `NOT EXERCISED: <reason>`. Rendered web/hub cells also write the
   [evidence bundle](references/evidence-bundle.md) to `~/.cas/artifacts/<project-key>/<task-id>/qa/`,
   with its trace, ARIA and polish requirements even when `visual_change: false`. A visual change
   adds [before/after pairs](references/evidence-bundle.md#beforeafter-pairs) from
   `cas deliverable capture --task <task-id> --target <target> --side both`. Cite
   `qa-bundle: <abs>/qa/bundle.json` in a `platform_proof` note and the close reason. API-only
   changes redact credentials and record the sandbox reset and the zero-smoke-rows check inline.
6. Grep the touched feature for `MIN_`, `MAX_`, `_MINUTES`, `_MS`, `_SECS`, `THRESHOLD`, `GRACE`,
   `DEBOUNCE` and `RETRY`; record whether each constant is predictable from the user's visible
   contract. For terminal states, dump all visible text across surfaces and flag contradictory
   claims. Run the [walk rules](#walk-rules) sweep for controls the user can't act on.
7. Run `cas qa ledger check <path> --task <id>` and repair every diagnostic. File one task per
   defect; implementers fix their own delivery. Add the ledger path, build revision, label split
   and verdict counts to a task note and the `task action=close` reason. Stop registered servers
   before close.

## Walk rules

These hold for every walk, task pass or [independent pass](references/independent-pass.md).

- **Every entry path to a shared step.** Before a verdict on a step several flows share (signup,
  identity, claim, checkout), list every entry path reaching it from source routes and any journey
  catalog. Walk them in order of real traffic: the path that supplies the changed input early
  passes while the rest break. Entry paths take the first rows; any past the cap stay
  `NOT EXERCISED: <reason>`.
- **Grade the visible result first.** Cite a spec `path:line` and quote per expectation;
  uncited expectations are spec questions. Wait for real reply text, record measured reply time
  and open delivered artifacts. Storage read-back only corroborates. Use one row per defect across
  widths/channels; run `ifAsked` answers only for the current open question.
- **Corroborate persisted invariants** from the system of record (database query, API read, admin
  view) after the final step, not from the screen. Grade paid/account routes and adjudicate
  harness-only reds with [server read-back](references/evidence-ledger.md#server-read-back-and-harness-failures);
  put the read and result in `observed`.
- **Map the build.** For a served app, record the backend and frontend SHAs served at the start and
  end of each walk; disclose a mixed build in the ledger header. A backend-only merge ships no new
  frontend build: prove it with `git diff --stat <served-frontend-sha> <tip> -- <frontend paths>`.
- **Sweep for controls the user can't act on:** an instruction without a control, an unselectable
  option, a mislabelled control, a dead end, lost intent between steps, an unreachable route, a
  stale overlay. Each is a defect row.
- **Keep fixtures valid.** A single-use token (invite, magic link, one-time code) is spent by the
  first attempt: mint a fresh one rather than retrying in a new browser. Use locale-valid country,
  phone and address fixtures, never reserved or example ranges.

## Close gate

In `qa.mode=per_task`, close (`qa.evidence_gate`) requires fresh commit-bound evidence: rendered
surfaces need the cited bundle with its passing Expect, visual QA, critique and base-build
comparison; API-only changes need `LEDGER.md` and `contracts.diff`; demo-only non-web changes need a
real-build PASS ledger, and terminal-render paths also need cas-cli-craft `terminal-qa: PASS`. In
batch mode, close of an integrated delivery waits for the release batch receipt instead.
`<skills-dir>` is the harness skill directory (`.claude/skills`, `.codex/skills` or
`.grok/skills`); use its existing evidence tools rather than a second format.

Every finding records severity, reach, journey and runtime repro in its triage; Cassy derives the
priority, so code-read-only findings land as theoretical backlog. Reproduced main-journey FAILs come
first. Honor the inherited operator focus.

## Boundaries

See the [worked matrix](references/exemplar.md), and check the delivery's `design:` against
[DESIGN.md first](../design-spec/references/design-md-first.md). `cas-playwright-debug` owns
diagnosis, `cas-servers` process lifecycle, the task verifier judgment. Log automation-caused
artifacts in the ledger's Honesty section; never modify the verifier to pass a missing capture.

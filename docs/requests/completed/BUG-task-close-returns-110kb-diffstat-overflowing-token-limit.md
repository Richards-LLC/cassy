---
from: ozer factory — session ozer-warm-owl-12, supervisor lively-dolphin-39
date: 2026-07-27
priority: P2
cas_task: (none)
---

# `task close` returns a ~110KB diff stat vs `main`, overflowing the tool-result token limit every time

## Symptoms

Every `task action=close` on this project returns a `📊 Committed diff stat (vs main)` listing **every file** that differs between the closing branch and `main`. On a long-lived epic branch that is ~110,000 characters / ~1,700 lines, which exceeds the MCP tool-result token limit. The result is spilled to a file, and the caller receives an error-shaped response plus a multi-paragraph instruction to read the spill file in chunks and state which portion was read.

The close itself **succeeds** — but the caller cannot tell that from the response, because the success line (`Closed task: cas-XXXX`) is the first line of a file that now looks like a failure.

## Concrete evidence

Six closes in one session, each spilling ~110KB:

| Task | Result size |
|---|---|
| cas-5a7a | 110,058 chars / 1,698 lines |
| cas-9923 | 110,220 chars / 1,701 lines |
| cas-93f1 | 110,036 chars / 1,698 lines |
| cas-7ffc | 110,112 chars / 1,699 lines |
| cas-9ec6 | 109,533 chars / 1,691 lines |
| cas-8f6a | 109,987 chars / 1,698 lines |

Sampling one spill file confirms the payload is almost entirely irrelevant to the task being closed — `.claude/CODEMAP.md`, `.claude/skills/backend-dev/**`, `.cursor/rules/**`, and hundreds of similar entries. The closed task touched six frontend files.

Each occurrence costs an extra shell call to read the head of the spill file and confirm the close actually landed.

## Likely root cause

The diff stat is computed against `main`, not against the task's own base or the epic branch. On a repo whose `staging` has diverged substantially from `main`, that is the entire divergence, not the task's contribution. The output is also not truncated before being returned.

## Proposed fix

- a) **Diff against the right base (leaned).** Compute the stat against the task's epic branch (or the branch point), not `main`. That yields the handful of files the task actually changed — which is presumably the intent of showing a diff stat at close.
- b) Truncate. Cap at N files with an `… and M more` line. Useful regardless of (a), since a genuinely large task should not overflow either.
- c) Put the success line first and make it survivable — or omit the diff stat from the tool result entirely and expose it via `task show`. The current shape is the worst case: a successful operation that presents as an error and requires follow-up work to confirm.

(a) plus (c) together would remove the cost entirely.

---

## Resolution

**Resolved 2026-07-27 — shipped in cas v2.29.0 (`origin/main`).** Task: cas-e093

Same root cause as the zero-commit catch-22 — merge-base computed against `main` across a diverged staging branch produced a 110KB stat. Parent-branch resolver unified and the diff stat bounded in `7acb9b6` (merge `01348e7`).

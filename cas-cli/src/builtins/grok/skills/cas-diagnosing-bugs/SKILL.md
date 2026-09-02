---
name: cas-diagnosing-bugs
description: Use when diagnosing, debugging, or reproducing a broken, failing, throwing, or slow behavior.
managed_by: cas
license: MIT
metadata:
  author: Matt Pocock
  upstream: https://github.com/mattpocock/skills
  provenance: Adapted from mattpocock/skills (MIT, © 2026 Matt Pocock).
---

# Diagnosing bugs

Use a feedback-loop-first discipline. Skip a phase only with an explicit reason.
Redact every secret in commands, output, and artifacts; retain the signal and ask
for a redacted artifact or access when redaction prevents diagnosis.

## Phase 1 — Build a tight, red-capable loop

Do not form a causal theory before one command can reproduce the user's exact
symptom. Prefer, in order: a failing scoped test; a CLI fixture; an HTTP or
browser assertion; captured-trace replay; a minimal harness; property/fuzz or
bisection loop; differential run; then a human-in-the-loop runbook. Treat the
loop as a product: make it fast, deterministic, and specific. For flakes,
increase reproduction rate with repeated or stress runs.

Completion requires one already-run command whose redacted output proves it is
red-capable, deterministic (or has a stated high repro rate), fast, and
unattended. If no loop can be built, state what was tried and request the
reproducing environment, a redacted capture, or approval for temporary
instrumentation; do not hypothesize without a loop.

## Phase 2 — Reproduce and minimize

Run the loop, confirm it is the user's failure rather than a nearby one, and
capture the symptom. Remove inputs, callers, configuration, and steps one at a
time until every remaining element is load-bearing.

## Phase 3 — Rank falsifiable hypotheses

Produce 3–5 ranked hypotheses. Each must predict what changing one variable
would do. Record them with `cas__task action=notes note_type=discovery` and
invite domain correction without blocking on it; discard a hypothesis that
cannot make a testable prediction.

## Phase 4 — Instrument one prediction at a time

Prefer debugger/REPL inspection, then targeted boundary logs. Tag temporary
logs with a unique `[DEBUG-…]` prefix. For performance regressions, establish a
measurement or profile baseline before changing code.

## Phase 5 — Fix and regression

Turn the minimized repro into a failing test only at a seam that exercises the
real call-site pattern. If no correct seam exists, record that architectural
finding. Otherwise: make the regression fail, fix it, make it pass, then rerun
the original loop.

## Phase 6 — Cleanup

Before claiming done, rerun the original loop, confirm regression coverage (or
the documented missing seam), remove tagged instrumentation and marked
throwaways, and record the validated hypothesis in the commit message and with
`cas__task action=notes note_type=discovery`.

---
name: cas-diagnosing-bugs
description: Use when diagnosing, debugging, or reproducing a broken, failing, throwing, or slow behavior.
license: MIT
metadata:
  managed_by: cas
  author: Matt Pocock
  upstream: https://github.com/mattpocock/skills
  provenance: Adapted from mattpocock/skills (MIT, © 2026 Matt Pocock).
---

# Diagnosing bugs

Use a feedback-loop-first discipline; skip a phase only with an explicit reason. For a repeated failed fix, use [principles.md](../cas-codebase-design/references/principles.md) to challenge the premise. Redact every secret in commands, output, and artifacts. Build loops on environment
variables so credentials remain in the environment; show the variable names,
not their values. Quote only signal lines from captures, which may contain auth
headers; ask for a redacted artifact or access when redaction prevents diagnosis.

## Phase 1 — Build a tight, red-capable loop

Do not form a causal theory before one command can reproduce the user's exact
symptom. Prefer, in order: a failing scoped test; a CLI fixture; an HTTP or
browser assertion; captured-trace replay; a minimal harness; property/fuzz or
bisection loop; differential run; then a human-in-the-loop script. For that last
branch, copy and edit [scripts/hitl-loop.template.sh](scripts/hitl-loop.template.sh),
then run the copy; capture observations and keep sign-in in a human-only step.
Treat the loop as a product: make it fast, deterministic, and specific. For flakes,
increase reproduction rate with repeated or stress runs.

Completion requires one already-run command whose redacted output proves it is
red-capable, deterministic (or has a stated high repro rate), fast, and
agent-runnable (human steps use the structured template). A factory worker runs a targeted Rust loop under `cas-worker` discipline:
commit the reproducer, select one package and a mandatory named-test `-E`
filter, then use the capped nextest runner. Commit the fix and rerun that
filter. The full suite stays at assembly. If the installed runtime denies
this exception, use captured logs, an installed CLI or a script, or hand the
committed reproducer to the supervisor with the exact denial. If no loop can be built, state
what was tried and request the reproducing environment, a redacted capture, or
approval for temporary instrumentation; do not hypothesize without a loop.
When a denied Rust reproducer is the chosen loop, wait for the supervisor's
executed command and redacted output before Phase 2; an already-executed non-Rust
loop that proves the exact symptom can satisfy the same criterion.
Package-scoped compile checks use the capped runner but cannot supply red/green
execution evidence; broad Rust builds and suites remain restricted.

## Phase 2 — Reproduce and minimize

Run the loop multiple times, confirm it is the user's failure rather than a nearby
one, and capture the symptom and the reproduction rate for a flaky failure. Remove inputs, callers, configuration, and steps one at a
time until every remaining element is load-bearing.

## Phase 3 — Rank falsifiable hypotheses

Produce 3–5 ranked hypotheses. Each must predict what changing one variable
would do. Record them with `task action=notes note_type=discovery` and
invite domain correction without blocking on it; discard a hypothesis that
cannot make a testable prediction.

## Phase 4 — Instrument one prediction at a time

Prefer debugger/REPL inspection, then targeted boundary logs that distinguish
hypotheses; never “log everything and grep”. Tag temporary logs with a unique
`[DEBUG-…]` prefix. For performance regressions, establish a timing or profile
baseline, then bisect the regression with that measurement before changing code.

## Phase 5 — Fix and regression

Turn the minimized repro into a failing test only at a seam that exercises the
real call-site pattern. If no correct seam exists, record that architectural
finding. Otherwise: make the regression fail, fix it, make it pass, then rerun
the original loop.

## Phase 6 — Cleanup

Before claiming done, rerun the original loop, confirm regression coverage (or
the documented missing seam), remove tagged instrumentation and marked
throwaways, and record the validated hypothesis in the commit message and with
`task action=notes note_type=discovery`.

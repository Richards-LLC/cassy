# Design record: per-task depth (speed mode for feel-driven iteration)

**EPIC:** cas-1255 · **Date:** 2026-06-25 · **Status:** shipped on the epic
branch (children cas-0344, cas-a63e, cas-6538, cas-bdab, cas-9d74)

## Why this exists

The trigger was a concrete friction Daniel and Ben hit during feel-driven UI
iteration: a pass that a human can evaluate by *looking at localhost* in about
**11 minutes** was taking roughly **50 minutes** when it went through Cassy's full
close rigor — verification jail, the P0 code-review / supervisor-review gate, and
the worker's pre-close self-checks. For logic work that rigor is exactly what you
want. For "is the spacing right, does this feel better?" work, the machine has
nothing useful to say — the human eye is the evaluator — so every gate is pure
latency.

Per-task depth lets the human opt a single task out of machine verification when
*they* are the test, without weakening the default for everything else.

## The mental model

Two lanes, default-safe:

- **deep** (default, and what unset/legacy tasks read as): full execution rigor,
  unchanged. This is the overwhelming majority of work.
- **light**: a fast, feel-driven pass. At close time it skips the two *rigor*
  gates (verification jail + the P0 review hop) and the worker's pre-close
  self-checks, and records an auditable decision note saying what was skipped and
  why. The human who asked for the light task is the evaluator.

Crucially, `light` only relaxes *review rigor*. Data-state safety guards
(merge-state, uncommitted-work, additive-only, commit-claim) and the supervisor's
explicit `bypass_code_review` override are orthogonal and untouched.

## How it was built (the lanes)

The feature was deliberately split so each layer is independently testable:

1. **Data model — cas-0344.** `TaskDepth` enum (`Deep` default + `Light`) in
   `cas-types`; a `depth` column in `cas-store` with `NULL → Deep` on read so
   pre-existing rows keep reading as deep; an idempotent `ALTER` migration
   registered in both runners (m202 cas-cli live runner + m122 cas-core); the
   MCP `task` create/update surface accepts `light|deep` and rejects anything
   else; depth is surfaced in `task show` and `task mine`.
2. **Close gate — cas-6538.** A single `depth == Light` flag in `close_ops.rs`
   gates three sites: the verification jail, the supervisor-review transition
   (which *is* the P0 gate under `owner = "supervisor"`), and the code-review
   gate routing. A self-contained decision note is written on every light close.
   `deep`/unset is byte-for-byte unchanged.
3. **Worker discipline — cas-a63e.** A "Task Depth" section in the `cas-worker`
   skill (the `include_str!` SessionStart source plus the synced `.claude`/
   `.codex` copies) tells a worker on a light task to make a minimal diff, skip
   the pre-close self-checks, and stop on localhost for the human to evaluate.
   Depth is read from the task record, not the environment.
4. **Capstone — cas-9d74.** End-to-end tests that prove the data layer and the
   close gate *compose* across a real SQLite persistence boundary (create in one
   session → reload depth from a fresh store → close in a separate session), plus
   the user guide and this record.

## Known boundary (intentional)

The dispatch-layer MCP jail (`authorize_agent_action` /
`check_pending_verification`) is a *separate* enforcement layer that blocks solo
(non-factory, non-supervisor) closes before `cas_task_close` runs. It does **not**
honor `depth=light`. For the documented factory-worker scenario (default
`owner = "supervisor"`) that layer is already exempt, so light closes are instant
end-to-end. A *solo* light close would still hit the dispatch jail — if instant
solo light closes are ever wanted, that needs a separate depth-light exemption in
`authorize_agent_action`. Flagged here so a future reader doesn't mistake it for a
bug.

## For future readers

If you're tempted to make `light` skip more (e.g. the data-state guards), don't —
those guards protect the repository, not review rigor, and a light task can still
corrupt a branch. The whole feature is scoped to "the human is the evaluator for
*this* task", nothing more.

See [`docs/guides/task-depth.md`](../guides/task-depth.md) for the usage-facing
version.

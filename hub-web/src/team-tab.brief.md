# Brief: Team beside the conversation

## Single idea

The operator can see who is working on the supervisor's epic, what needs a decision, and whether delivery is advancing without opening a terminal or reading a stream.

## Hero form

A compact epic progress rule above a staffing ledger: one lane per live worker, a shared left edge for task/state, and a short evidence trail toward delivery. On a phone the same lanes stack; no desktop diagram shrinks into illegibility.

## Emotional register

Calm and exact Commander Glass. Reuse hub-web/DESIGN.md and tokens.css/glass.css roles for ink, action, good, warning, critical, rule, focus and spacing. State text and semantic marks carry meaning together; color and movement never carry it alone.

## Distinctive move

The lane joins the worker to its current task and the actual delivery evidence. Started, pushed, CI green, merged and verified are distinct facts; absent evidence stays unreported. A small Needs you strip leads back to the existing ask, blocker or merge action rather than creating another operations console.

## Deliberately omitted

No terminal, PTY, raw output, tool-call text, streaming transcript or guessed progress. No new polling owner or independent epic selection. No percentage inferred from elapsed time, no CI success inferred from a push, and no verification inferred from a closed task.

## Shared state and boundaries

Reuse progress-model.ts workerProgress, releaseProgress and taskGroups, which already back Tasks & progress. The side-panel redesign is closed (cas-5a42); its current shared model is authoritative. main.ts owns selected machine/session, current catalog/status and event invalidation. A lazy Team view owns only its DOM, tab interaction and animation state. Any extra pure presentation projection belongs beside the existing shared model so the rail and Team cannot disagree.

The Team tab uses the existing authenticated status/event path and its coalesced refresh. A display-only elapsed-time clock must never fetch data. Switching conversation, losing a pairing or closing Team cancels ownership; stale/offline data has explicit freshness. The selected session's catalog roster wins over unrelated agents.

## Reported data contract

The daemon supplies additive FleetAgent harness/model from worker_cli/worker_model registry metadata, task_started_at from bounded task-start events, and recent_trail from durable task delivery and verification records. Registration and heartbeat never stand in for time on task. Local commits do not prove push; advisory/override merges do not prove CI green; Closed does not prove verification. A current approved verdict after the task start is required for a verification mark. Missing fields on older hubs remain Not reported. Native liveness reuses worker_status; Team and the rail share delivery/liveness precedence in workerState. The supervisor reserves a read-only real-hub comparison with four live workers per #4216909.

## Acceptance and proof

Few unit cases prove shared scoping/state/evidence rules; extend existing progress-model tests first. UI tests cover lazy tab ownership, event-driven updates, empty/error/stale states and no raw-output subscriptions. Add a catalog journey for Team and run app/e2e tsc plus producer-selected journeys against a task-only preview-dist; commit no generated dist, per supervisor #4216882. The supervisor-approved named config at target/cas-1152-preview.config.mts points at registered team-preview and task-only preview-dist. Committed-dist receipt tooling cannot identify this snapshot; the ledger records its actual source revision and snapshot hash, with the tooling gap tracked under cas-6cb7.

Real-hub proof compares the same three live workers and epic snapshot to the Team view, then changes one worker and observes it within five seconds without reload. Capture before/after, 1280 and 390, light/dark, forced colors and reduced motion. Score with the craft rubric; every public-surface floor must reach four before delivery.

## Implementation and state

team-view.ts owns the lazy DOM and display-only elapsed clock; main.ts owns selection, status/errors, shared event invalidation and tab bindings. No component fetches, subscribes to raw output or writes factory state. Loading, status failure/retry, disconnected last-known, empty roster and ended-session states have explicit text. Keyboard arrows/Home/End select tabs and return focus. The conversation remains mounted and retains its draft. Needs you uses existing conversation/fleet controls.

The delivery trail uses semantic ordered lists, labeled receipt marks and text; forced colors preserve the marks, and reduced motion removes the brief state animation. Desktop lanes become one column at phone width. The Team renderer is a separate lazy chunk. Initial app gzip is 201,286 bytes versus 200,266 bytes in the committed baseline (1,020-byte increase); this is below the 200 KiB route budget (204,800 bytes). No images are required.

## Critique

Pending rendered evidence and scored rubric.

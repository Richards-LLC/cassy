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

## Data contracts to confirm

FleetTask already has epic_id, delivery_state and epic progress; FleetAgent has current_task, liveness, latest_activity, started_at and heartbeat, but its typed interface has no harness/model or authoritative task-start/lifecycle receipts. Worker started_at is not time on the current task. Confirm additive native fields or the panel-data owner before implementing those facts; unsupported older hubs must say Not reported. Supervisor readback/fixture setup is needed for acceptance comparison with factory worker_status and epic_status and at least three real live workers.

## Acceptance and proof

Few unit cases prove shared scoping/state/evidence rules; extend existing progress-model tests first. UI tests cover lazy tab ownership, event-driven updates, empty/error/stale states and no raw-output subscriptions. Add a catalog journey for Team and run app/e2e tsc plus producer-selected journeys against a task-only preview-dist; commit no generated dist, per supervisor #4216882. The current serve-dist and journey receipt contract need inspection for the supported preview input before browser runs.

Real-hub proof compares the same three live workers and epic snapshot to the Team view, then changes one worker and observes it within five seconds without reload. Capture before/after, 1280 and 390, light/dark, forced colors and reduced motion. Score with the craft rubric; every public-surface floor must reach four before delivery.

## Critique

Pending implementation and rendered evidence.

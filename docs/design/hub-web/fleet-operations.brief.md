# Brief: structured fleet operations in Commander (cas-9886)

Status: design only, no implementation. The supervisor turns the slices below
into tasks.

## Single idea

From a supervisor conversation, the operator can do the handful of things to
the fleet that today mean typing into a terminal or asking the supervisor:

- add or stop a worker;
- pause a worker;
- point the factory at an epic;
- hand a ready task to a worker;
- ask the supervisor to merge something that is waiting.

Each action is one deliberate tap. It runs through the same Cassy function and
audit trail as the CLI/MCP action of the same name, and it fails safely when
the fleet has moved on.

## What exists today (facts, with citations)

Paths are relative to the repo root.

### The hub's mutating surface

**HTTP routes** (`cas-cli/src/hub/server.rs:89-160`):

| Route | Scope | What it does |
| --- | --- | --- |
| `POST /v1/sessions` | `session:launch` | Launch a session (`launch_session`, `:687` → `spawn_factory_daemon`, `:1205`). |
| `DELETE /v1/sessions/{s}` | `factory:manage` | End a session (`end_session`, `:730` → `cli::factory::end_session_by_name`, `lifecycle.rs:146`). |
| `POST /v1/sessions/{s}/lease`, `DELETE /v1/sessions/{s}/lease` | `pane:input`, or `hub:admin` to force | Take or release control. |
| `POST /v1/auth/scopes` | — | Self-grant. Only `session-launch` can be added (`auth.rs:1197`). |

**Multiplex WebSocket frames** (`handle_client_message`, `server.rs:1823`). The
scope comes from `required_scope()` (`auth.rs:193-223`), and every mutation also
needs the active lease (`server.rs:1839`). The hub forwards each frame to the
session daemon:

| Frame | Scope |
| --- | --- |
| `Input`, `Focus`, `Resize` | `pane:input` |
| `SendMessage` | `message:send` (durable queue, `delivery.rs:585`, deduplicated by `client_ref`) |
| `InterruptPane` | `pane:interrupt` |
| `SpawnWorkers`, `ShutdownWorkers`, `Inject`, `SpawnShell`, `KillShell` | `factory:manage` (`ui/factory/daemon/runtime/ws_client.rs:502-609`) |

The hub-web client never sends the `factory:manage` frames.

**The two worker-spawn routes are not the same function.** The WebSocket
`SpawnWorkers` frame feeds the daemon's `PendingSpawn` queue directly. It
cannot pre-assign a task (`daemon/runtime/ws_client.rs:512`). The MCP
`factory_spawn_workers` (`mcp/tools/service/factory_ops.rs:2182`) goes through
`enqueue_spawn_with_requester_account_dirs`, with task pre-assignment and the
requester's account dirs. The acceptance criterion says to reuse the same
function as the CLI/MCP action, so Commander must not build on the WebSocket
frames.

**Audit trail.** Audit rows go to append-only `audit.jsonl` (`auth.rs:586`;
writers at `:1406`, `:1422` and `:1454`).

- Audited today: every WebSocket frame, and launch (a requested row, then an
  allowed row with placement; the launch is refused if the audit write fails).
- Not audited: End session (only `tracing::info!`, `server.rs:766`), and lease
  acquire and release.

**Fleet state reaches the browser by pull only.**

- `GET /v1/sessions/{s}/status` (`server.rs:1410` → `build_status_json`,
  `bridge/server/session.rs:70-130`) returns `agents`, `tasks_ready`,
  `tasks_in_progress` and `epics`.
- hub-web refetches it on attach and on every machine event for the open
  session (`main.ts:1665`, `:892`, `loadStatus` `:1864`).
- `/v1/events` carries session and pane lifecycle only (`events.rs:17-26`).
- There is no fleet push, and none is needed: a `FleetChanged` machine event
  can prompt the same refetch.

### The Cassy functions to reuse

All of these are MCP-only today. They are `CasService` methods, `pub(super)`,
reached through MCP dispatch, and there is no `cas task` CLI.

| Operation | Function |
| --- | --- |
| Spawn workers | `CasService::factory_spawn_workers` (`factory_ops.rs:2182`) |
| Shut down workers | `factory_shutdown_workers` (`:2804`) |
| Recycle a worker | `factory_recycle_worker` (`:3142`) |
| Hold or resume a worker | `factory_set_worker_hold` (`:3362`) |
| Worker status | `factory_worker_status` (`:3492`) |
| Focus an epic | `factory_focus_epic` (`:6407`); `factory_epic_status` (`:6240`) |
| Ready tasks | `task_ready` (`service/core.rs:575` → `CasCore::cas_task_ready`, `core/task/lifecycle.rs:1383`) |
| Assign a task | `task_update` with `assignee` (`service/core.rs:403` → `core/task/update.rs:358`, assignee at `:1210`) |
| Ask the supervisor | `message_send` (`service/agent_search_system/message.rs:613`) |

### Permissions in the browser today

- Pairing understands `PAIRING_SCOPES` plus `session-launch` only
  (`hub-web/src/pairing-scopes.ts:5,15`).
- An invitation that names `factory-manage` or `hub-admin` fails
  `parseGrantedScopes` and falls back to read-only (`:30-46`). So no real
  browser can hold `factory:manage`. The e2e double injects it
  (`e2e/journeys/end-session.journey.ts:22`).
- End session shows only when the device holds `factory-manage`
  (`main.ts:4058`).

## Operations in scope

| # | Operation | Where the operator meets it | Cassy function (reused, not re-implemented) | Kind |
| --- | --- | --- | --- | --- |
| O1 | Ask the supervisor to merge | An `awaiting_merge` task in Tasks & progress or in Attention (already derived, `main.ts:1873`) | `message_send` to the session's supervisor. The body is generated from the task (`id`, branch, tip) and sent as an explicit operator turn. | Request, not mutation |
| O2 | Focus an epic | Tasks & progress header: "Focus epic…" | `factory_focus_epic` | Reversible |
| O3 | Add workers (N, optionally on a ready task) | Agents section: "Add worker…" | `factory_spawn_workers` (count, optional `task_id`) | Additive |
| O4 | Pause / resume a worker | Agent row menu | `factory_set_worker_hold` (hold / release) | Reversible |
| O5 | Assign a ready task to a worker / unassign | Ready-task row: "Assign…", with a picker of idle workers | `task_update` with `assignee`, the same call the supervisor makes | Reversible |
| O6 | Restart a worker | Agent row menu | `factory_recycle_worker` | Destructive (loses in-flight context) |
| O7 | Stop a worker | Agent row menu | `factory_shutdown_workers` (graceful; force only behind a second confirmation) | Destructive |
| O8 | End the session | Exists (row End session) | `end_session_by_name`. Add the missing audit row. | Destructive |

**Deliberately omitted:**

- Browser-run git merges, or any merge action. O1 asks the supervisor.
- Starting a factory beyond the existing New session.
- Editing tasks.
- Injecting raw text into a worker. `Inject` stays CLI/MCP-only.
- Any automatic or background model turn. Only O1 produces a model turn, and
  only when the operator taps it.
- Polling. Refreshes are driven by events and responses.

## Server design (one path, audited, stale-safe)

### 1. An operator facade, shared by MCP and the hub

Move the bodies the MCP actions above already call into one `pub(crate)`
facade, `cas-cli/src/ops/fleet.rs`. Each method takes:

- an `Actor`: MCP agent, or hub device with its labels;
- an `expected` precondition;
- an idempotency key.

Both the MCP dispatch arms and the hub endpoint call the facade, so the audit
semantics are the same by construction. MCP keeps its current
request/response shapes, and the existing MCP tests stay the regression net.

### 2. One hub endpoint

`POST /v1/sessions/{s}/operations` takes:

```json
{ "op_id": "…", "op": { "kind": "…" }, "expected": { } }
```

- `op_id` is a client UUID. The hub keeps a short-lived `op_id → outcome`
  map, the HTTP analogue of the `client_ref` dedupe on `SendMessage`. A retry
  after a dropped response returns the first outcome and never runs twice.
- `expected` carries the state the operator saw:

  | Operation | Precondition |
  | --- | --- |
  | Worker ops | worker name plus its spawn generation (or `started_at`) |
  | O5 | the task's `updated_at` and current assignee |
  | O2 | the current epic id |
  | O1 | the task status `awaiting_merge` and its tip SHA |

  A mismatch returns `409 {"error":"stale","current":{...}}` and changes
  nothing. The UI shows what changed ("swift-lark-3 already restarted") and
  refreshes.
- Errors follow the existing shape `{"error":code,"detail":...}`, as in
  launch.
- hub-web's `request()` must surface `code` and `detail` instead of
  `Error("… failed (status)")`. Today it throws the body away
  (`connection.ts:504-519`).
- **Audit:** a `requested` row before the call and an `outcome` row after it,
  through `write_audit`. The action is refused with `audit_unavailable` if the
  requested row cannot be written. This is the launch rule (`server.rs:1061-1104`).
- After a successful operation the hub emits a `FleetChanged { session }`
  machine event. Every connected device refetches status. There is no new
  polling.
- **Lease:** a structured operation does not need the session control lease. It is
  not typing into a pane. It still needs the scope and a live device
  credential, which are rechecked at execution time as launch does
  (`server.rs:1049`).

### 3. Scopes

"Destructive actions require … distinct scopes", so the scopes are split:

| Scope | Grants | How a browser gets it |
| --- | --- | --- |
| `message:send` (existing) | O1, ask to merge: it is an explicit supervisor message | Existing control pairing |
| `factory:operate` (new) | O2 focus, O3 add workers, O4 pause/resume, O5 assign | `cas hub pair --scopes …,factory:operate`, or a one-time in-browser grant like session launch (`auth.rs:1197`), allowed only when the device already holds the control scopes |
| `factory:manage` (existing) | O6 restart, O7 stop, O8 end session | Pairing invitation only, never self-granted. A machine owner must mean it on the machine. |
| `hub:admin` (existing) | Unchanged (forced lease, revoke) | Pairing only |

**Browser side:**

- `parseGrantedScopes` learns `factory-operate` and `factory-manage`
  (`pairing-scopes.ts`).
- The consent copy names them in plain words: "Manage workers and tasks" and
  "Stop and restart workers and sessions"; Paired machines adds that it also
  allows write grants (cas-ab04, cas-a217).
- Each control renders only when its scope is held.
- A missing scope shows the control disabled with the command that grants it,
  the same pattern as the read-only pair link (cas-b52d).

## Interaction design

### 1280 (desktop, context rail visible)

- **Tasks & progress** keeps its rows (`main.ts:4577-4627`). Each row gains
  one trailing control.
- **Agent row:** a "⋯" menu button (`aria-haspopup="menu"`) with Pause or
  Resume, Restart…, and Stop…. Disabled items say why ("Needs Stop and restart
  permission").
- **Ready task row:** an "Assign…" button opens a small popover listing idle
  workers (name and status chip). Choosing one assigns.
- **Awaiting-merge task row**, and the same item in Attention: one button,
  "Ask supervisor to merge". It shows the exact message it will send before
  sending. Sent, it becomes "Asked 2m ago" and links to the turn in the thread.
- **Section header:** "Add worker…" (count stepper, 1-4, and an optional
  "Start on" ready task) and "Focus epic…" (a list of the status `epics`).

**Confirm.** Destructive operations (O6, O7) reuse End session's inline
confirmation (`conversation-list.ts:401-458`):

- It names what stops: "Stop swift-lark-3? Its task cas-1234 goes back to
  ready."
- Cancel is placed and focused first.
- A double-click's second press is ignored.
- The confirm button is `danger`.
- Force stop is a second step, offered only after a graceful stop fails.

**Undo.** Reversible operations (O2, O4, O5) take effect at once and show a
toast with Undo for 8 s. Undo issues the inverse operation with its own
`op_id` and `expected`, so it can itself come back `stale`. It is not a
delayed send. Additive O3 and destructive O6/O7 have no Undo: O3's toast names
the new workers, and O6/O7 are confirmed instead.

**Progress and failure.** The row shows "Stopping…" as a `role="status"` line,
and focus moves there, as End session does (cas-e634). On failure the row
shows the hub's `detail`. On `stale` it shows what changed and refreshes.

### 390 (phone)

- The rail is the "Tasks & progress" sheet. Row controls become one trailing
  44×44 "⋯" per row, opening a bottom action sheet. Each action is a full-width
  44 px row. Destructive actions are last, in `danger`, separated.
- **Confirm** is a second sheet step:
  - the question in plain words;
  - Cancel (full width, first);
  - the destructive button.
  - It is never a hover popover.
- **Undo** toasts sit above the composer and clear the keyboard inset
  (`bindKeyboardViewport`).
- **Assign** and **Focus epic** pickers are full-height sheets with search.
- **Ask supervisor to merge** shows the message preview in the sheet before
  Send.

### Both widths

- Keyboard: Escape closes a menu or sheet and returns focus to its opener.
- Every result is announced in a live region.
- Forced colours: menus and confirm states keep `Highlight` outlines.
- Reduced motion: sheets appear without sliding.
- Nothing refreshes on a timer. The `FleetChanged` event and the operation's
  own response drive every refresh.

## Sliced task plan

Each slice is deliverable on its own and leaves the product consistent.

### S1. Operator facade, operations endpoint, and audit for End session (Rust)

Extract `ops::fleet` from the MCP bodies for O1 and O2 first, the lowest risk.
Then:

- add `POST /v1/sessions/{s}/operations` with `op_id` dedupe, `expected` →
  409 `stale`, requested/outcome audit rows and `FleetChanged`;
- write the missing End session audit row;
- keep MCP behaviour byte-identical.

Deviation (cas-566b, supervisor-approved): O1 does not call MCP `message_send`, whose sender comes from the MCP caller's registered agent, which a hub device lacks. It goes through the Commander lane, `ops::fleet::enqueue_commander_message`, and its row is pinned for parity with the MCP message row.

**demo_statement:** "A paired device asks the supervisor to merge an
awaiting-merge task through the hub. The supervisor receives the same message
an MCP `message` call sends, `audit.jsonl` has requested and outcome rows, and
a retried `op_id` sends nothing twice."

**proof targets:**

- hub server integration tests:
  - `operations_request_merge_reuses_message_send`
  - `operations_op_id_is_idempotent`
  - `operations_stale_expected_returns_409_without_side_effects`
  - `end_session_writes_audit_row`
- the existing MCP `message` and `factory focus_epic` tests, unchanged and
  green.

### S2. Worker lifecycle and the `factory:operate` scope (Rust)

Wire O3, O4, O6 and O7 through the facade and add `factory:operate`:

- parse, wire spelling, `required_scope` and audit scope;
- self-grant allowed for `factory:operate` only.

Destructive operations require `factory:manage`. Worker preconditions use the
spawn generation.

**demo_statement:** "With factory:operate a device can add and pause a worker
but gets scope_denied on Stop. With factory:manage it can stop the worker, and
a second Stop against the restarted worker's old generation returns stale."

**proof targets:**

- `operations_spawn_uses_factory_spawn_workers_queue`
- `operations_stop_requires_factory_manage`
- `operations_worker_generation_stale`
- `scope_factory_operate_roundtrip`
- the MCP `factory_*` tests unchanged.

### S3. Task assignment and epic focus (Rust)

O5 assign/unassign through `task_update`, with `updated_at` and current
assignee as preconditions. O2 is finished with an inverse for Undo.

**demo_statement:** "Assigning a ready task from the hub sets the same
assignee and task note as the supervisor's task update. Assigning it again
after another device assigned it returns stale with the current assignee."

**proof targets:**

- `operations_assign_task_matches_task_update`
- `operations_assign_stale_assignee`
- `operations_focus_epic_inverse`

### S4. Pairing and permission UX (hub-web)

- `parseGrantedScopes` and the consent copy learn `factory-operate` and
  `factory-manage`.
- The one-time "Allow managing workers" grant for `factory:operate` mirrors
  the session-launch grant flow.
- Controls are gated by scope, and a missing scope says which command grants
  it.
- `request()` surfaces `{error, detail}`.

**demo_statement:** "Opening a `cas hub pair --scopes …,factory:manage` link
shows 'Stop and restart workers and sessions' in plain words and grants it. A
control pairing without it sees Stop disabled with the command that adds it."

**proof targets:**

- vitest:
  - `pairing-scopes.test.ts` (new scope parse and summary)
  - `pair-dialog-markup.test.ts`
  - `connection.test.ts` (error body surfaced)
- HUB-J2 extended with a factory-manage link.

### S5. Desktop operations UI: rail controls, confirm, undo, ask to merge (hub-web, 1280)

The Tasks & progress row controls, the "⋯" menu, the inline destructive
confirmation, Undo toasts and the O1 message preview.

**demo_statement:** "At 1280 I pause a worker and undo it, assign a ready task
to an idle worker, stop another worker after confirming, and ask the
supervisor to merge an awaiting-merge task, seeing the exact message first.
Every result is announced, and nothing refreshes on a timer."

**proof targets:**

- new journey HUB-J17 "Run the fleet from a conversation" (desktop part),
  against the hub double extended with the operations endpoint;
- vitest for menu, confirm and undo state;
- visual-qa fixtures `fleet-ops-menu`, `fleet-ops-confirm` and
  `fleet-ops-undo`, strict, light/dark.

### S6. Phone sheets and the two-machine proof (hub-web, 390)

Action sheet, confirm step, keyboard-safe Undo, full-height pickers. Then the
end-to-end proof across two paired machines.

**demo_statement:** "On a 390 px phone paired to two machines, I add a worker
on one machine and stop a worker on the other, each from its own conversation,
with 44 px targets and a confirm sheet. A stale stop on the second machine
explains what changed. A real two-machine run records the same audit rows on
both hubs."

**proof targets:**

- HUB-J17 phone part (two machines in the double);
- a real-build platform proof on two hubs (`audit.jsonl` excerpts in the QA
  bundle);
- strict visual-qa at 390 and 844×390.

## Open questions for the supervisor

1. **Is a separate `factory:operate` scope acceptable?** The alternative is to
   reuse `factory:manage` for everything. That would let any manager stop
   workers and would not meet "distinct scopes for destructive actions".
2. **Should Restart (O6) require `factory:manage`, or is it closer to Pause?**
   This brief treats it as destructive, because it drops the worker's
   in-flight context.
3. **Should the operations endpoint require the session control lease?** This brief
   says no: it is not pane input. Requiring it would serialize operators in a
   way the CLI and MCP do not.

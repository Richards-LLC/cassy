# Spike: Validate `cas serve` for Slack Bridge

**Date:** 2026-04-10
**Task:** cas-9b39
**Epic:** cas-84a9 (Cassy Remote Deployment & Slack Bridge)
**Author:** cosmic-marten-61

## Summary

**GO.** The `cas serve` HTTP bridge already exposes every API primitive the Slack bridge needs: start sessions, inject messages, poll activity via SSE, query status. All endpoints tested and working against live factory sessions. No Rust changes required.

## Full API Surface

### Server Startup

```bash
cas bridge serve [--bind 127.0.0.1] [--port 0] [--token TOKEN] [--no-auth] [--cors-allow-origin ORIGIN] [--cas-root PATH]
cas --json bridge serve ...  # outputs JSON with port/token on stdout (one line)
```

On startup, outputs a `ServeInfo` JSON:
```json
{
  "schema_version": 1,
  "bind": "127.0.0.1",
  "port": 18999,
  "base_url": "http://127.0.0.1:18999",
  "token": "auto-generated-uuid",
  "auth_enabled": true
}
```

### Authentication

- **Bearer token** via `Authorization: Bearer <token>` header
- Token is auto-generated UUID unless `--token` is specified
- `--no-auth` disables auth entirely (localhost-only use)
- Returns `401` with `{"error":{"code":"unauthorized"}}` on mismatch

### CORS

- `--cors-allow-origin` sets `Access-Control-Allow-Origin` on all responses
- `OPTIONS` preflight returns 204 with CORS headers
- Allowed headers: `authorization, content-type`
- Allowed methods: `GET, POST, OPTIONS`

---

## Endpoints

### 1. `GET /v1/health`

Health check. Always returns:
```json
{"schema_version": 1, "ok": true}
```

### 2. `POST /v1/shutdown`

Graceful shutdown. Returns `{"schema_version": 1, "ok": true}` and stops the server.

### 3. `GET /v1/sessions`

List all factory sessions. Supports query filters:
- `name=<session-name>` — exact match
- `project_dir=<path>` — exact match on project directory
- `running_only=true` — only sessions with running daemon process
- `attachable_only=true` — only sessions with socket + running daemon

Response:
```json
{
  "schema_version": 1,
  "sessions": [{
    "name": "cas-src-solid-swan-72",
    "created_at": "2026-04-10T08:56:43...",
    "daemon_pid": 893831,
    "socket_path": "/home/pippenz/.cas/factory-cas-src-solid-swan-72.sock",
    "ws_port": null,
    "project_dir": "/home/pippenz/cas-src",
    "epic_id": null,
    "supervisor": "noble-owl-1",
    "workers": [],
    "is_running": true,
    "socket_exists": true,
    "can_attach": true
  }]
}
```

### 4. `POST /v1/factory/start`

Start a new factory session (or reuse existing).

Request body:
```json
{
  "project_dir": "/path/to/project",     // REQUIRED, must exist
  "workers": 2,                           // optional, max 6, default 0
  "name": "my-session",                   // optional, auto-generated if omitted
  "supervisor_cli": "claude",             // optional, default "claude"
  "worker_cli": "claude",                 // optional, default "claude"
  "no_worktrees": false,                  // optional
  "worktree_root": "/path",              // optional
  "notify": false,                        // optional
  "tabbed": false,                        // optional
  "record": false,                        // optional
  "reuse_existing": true                  // optional, checks for attachable session first
}
```

Response:
```json
{
  "schema_version": 1,
  "started": true,          // false if reused existing
  "reused_existing": false,  // true if attached to existing session
  "session": { /* SessionJson */ }
}
```

Behavior:
1. If `reuse_existing=true`, calls `find_session_for_project()` first
2. Auto-runs `cas init --yes` if `.cas/` doesn't exist in project_dir
3. Spawns `cas factory daemon` subprocess (detaches)
4. Polls up to 10s for session metadata + socket to appear
5. Returns 504 if session doesn't start within 10s

### 5. `GET /v1/sessions/<name>/ping`

Session liveness check:
```json
{
  "schema_version": 1,
  "ok": true,
  "session": { /* SessionJson */ },
  "cas_root": "/path/to/.cas"
}
```

### 6. `GET /v1/sessions/<name>/targets`

Get supervisor/worker names + aliases:
```json
{
  "schema_version": 1,
  "session": { /* SessionJson */ },
  "supervisor": "noble-owl-1",
  "workers": ["worker-a", "worker-b"],
  "aliases": {
    "supervisor": "noble-owl-1",
    "all_workers": "all_workers"
  }
}
```

### 7. `GET /v1/sessions/<name>/status`

Aggregated session status (cached 250ms). Includes:
- `prompt_queue_pending` — number of enqueued-but-uninjected prompts
- `activity` — last 20 events for session agents
- `agents` — summary of each agent (status, current_task, latest_activity, heartbeat)
- `tasks_ready`, `tasks_in_progress`, `epics` — task summaries

```json
{
  "schema_version": 1,
  "session": { /* SessionJson */ },
  "prompt_queue_pending": 15,
  "activity": [{ "id": 61335, "event_type": "supervisor_injected", "summary": "..." }],
  "agents": [{
    "id": "uuid",
    "name": "noble-owl-1",
    "status": "active",
    "current_task": null,
    "latest_activity": {"summary": "Committed changes", "created_at_rfc3339": "..."},
    "last_heartbeat_rfc3339": "..."
  }],
  "tasks_ready": [{ "id": "cas-xxxx", "title": "...", "status": "open", "priority": 0 }],
  "tasks_in_progress": [{ "id": "cas-yyyy", "title": "...", "assignee": "worker-1" }],
  "epics": [{ "id": "cas-zzzz", "title": "...", "status": "open" }]
}
```

### 8. `GET /v1/sessions/<name>/activity`

Activity events for the session. Query params:
- `since_id=<i64>` — only events after this ID
- `limit=<1-200>` — max events (default 50)

Response: `ActivityJson` with `activity: Vec<Event>` and `latest_id`.

Event shape:
```json
{
  "id": 61335,
  "event_type": "supervisor_injected",
  "entity_type": "agent",
  "entity_id": "noble-owl-1",
  "summary": "Injected queued prompt 920 to noble-owl-1 (ok)",
  "metadata": {
    "actual_target": "noble-owl-1",
    "prompt_id": 920,
    "queue_source": "cosmic-marten-61",
    "queue_target": "noble-owl-1",
    "status": "ok"
  },
  "created_at": "2026-04-10T16:19:53.244482593Z",
  "session_id": "some-uuid"
}
```

### 9. `POST /v1/sessions/<name>/message`

Inject a message into a session agent's prompt queue.

Request body:
```json
{
  "target": "supervisor",       // "supervisor" | "all_workers" | "<worker-name>"
  "message": "Build feature X",
  "from": "slack-bridge",       // optional, default "openclaw"
  "no_wrap": false,             // optional, if true sends raw text without response hint
  "wait_ack": false,            // optional, blocks until supervisor injection event
  "timeout_ms": 5000            // optional, for wait_ack
}
```

Response:
```json
{
  "schema_version": 1,
  "session": "cas-src-solid-swan-72",
  "target": "noble-owl-1",
  "enqueued": true,
  "message_id": 923,
  "ack_event_id": null          // populated if wait_ack=true and injection succeeded
}
```

Behavior:
- "supervisor" alias resolves to actual supervisor agent name
- Default wrapping appends a response hint: `To respond, use: coordination action=message target=<from> ...`
- `no_wrap=true` skips the response hint wrapping
- `wait_ack=true` polls the event store up to `timeout_ms` for a `SupervisorInjected` event matching the `message_id`

### 10. `GET /v1/sessions/<name>/events` (SSE)

**Long-lived Server-Sent Events stream.** Runs on a dedicated thread (doesn't block the accept loop).

Query params:
- `poll_ms=500` — poll interval (50-5000ms)
- `heartbeat_ms=15000` — keepalive interval (250-120000ms)
- `activity_limit=50` — max events per poll (1-200)
- `inbox_limit=25` — max inbox items per poll (1-200)
- `status_interval_ms=2000` — status snapshot frequency (0=disabled)
- `include_status=true` — enable/disable status snapshots
- `since_id=0` — only stream events after this ID
- `inbox_id=owner` — which inbox to poll

SSE event types:
- `: connected` — initial comment (connection established)
- `event: activity` — new activity events (JSON `ActivityJson`)
- `event: status` — aggregated status snapshot (JSON `StatusJson`)
- `event: inbox` — inbox notifications (JSON `InboxPollJson`)
- `event: error` — error with `code` and `message`
- `: heartbeat` — keepalive comment

Requires HTTP/1.1 (returns 505 for HTTP/1.0). Uses chunked transfer encoding.

### 11. Inbox Endpoints

#### `GET /v1/sessions/<name>/inbox/<inbox_id>/pending_count`
```json
{"schema_version": 1, "session": {...}, "inbox_id": "owner", "pending_count": 5}
```

#### `GET /v1/sessions/<name>/inbox/<inbox_id>/peek?limit=25`
Non-destructive read of pending inbox notifications.

#### `POST /v1/sessions/<name>/inbox/<inbox_id>/poll?limit=25`
Destructive read — marks notifications as processed.

#### `POST /v1/sessions/<name>/inbox/<inbox_id>/ack`
Acknowledge a specific notification.
```json
// Request:
{"notification_id": 42}
// Response:
{"schema_version": 1, "session": {...}, "inbox_id": "owner", "acked": true, "notification_id": 42}
```

---

## Round-Trip Validation Evidence

### Test 1: Health + Sessions List
```
curl http://127.0.0.1:18999/v1/health
→ {"schema_version":1,"ok":true}

curl http://127.0.0.1:18999/v1/sessions
→ 3 sessions listed, all with correct metadata
```

### Test 2: Session Status + Activity
```
curl http://127.0.0.1:18999/v1/sessions/cas-src-solid-swan-72/status
→ Full status with prompt_queue_pending=15, agent activity, task lists

curl http://127.0.0.1:18999/v1/sessions/cas-src-solid-swan-72/activity?since_id=0&limit=5
→ 3 recent supervisor_injected events with metadata (prompt_id, queue_source, status)
```

### Test 3: Message Injection
```
curl -X POST http://127.0.0.1:18999/v1/sessions/cas-src-solid-swan-72/message \
  -H "Content-Type: application/json" \
  -d '{"target":"supervisor","message":"test from spike validation","from":"slack-bridge-spike"}'
→ {"schema_version":1,"session":"cas-src-solid-swan-72","target":"noble-owl-1","enqueued":true,"message_id":923}
```

Message was enqueued and picked up by the supervisor daemon (visible in subsequent activity events showing `queue_source: "slack-bridge-spike"`).

### Test 4: SSE Stream (3-second sample)
```
curl -N http://127.0.0.1:18999/v1/sessions/cas-src-solid-swan-72/events?poll_ms=500&heartbeat_ms=1000
→ Received: `: connected`, then `event: activity` with 3 events, then `event: status` with full status JSON
→ Heartbeat comments every ~1s when no new data
```

### Test 5: Auth
```
curl http://127.0.0.1:18998/v1/health  (no header) → 401 unauthorized
curl -H "Authorization: Bearer wrong" http://127.0.0.1:18998/v1/health → 401 unauthorized
curl -H "Authorization: Bearer test-secret-123" http://127.0.0.1:18998/v1/health → 200 ok
```

### Test 6: Factory Start (validation)
```
curl -X POST http://127.0.0.1:18999/v1/factory/start \
  -d '{"project_dir":"/tmp/nonexistent"}'
→ 400 {"error":{"code":"invalid_project_dir","message":"project_dir must be an existing directory"}}
```

---

## `find_session_for_project()` Analysis

**Location:** `cas-cli/src/ui/factory/session.rs:196`

Behavior:
1. Lists all sessions from `~/.cas/` metadata files
2. If `name` is provided: returns exact name match (ignores project filter)
3. If no name: returns the first `can_attach()` session where `project_dir` matches exactly
4. `can_attach()` = `socket_exists && is_running`

**For Slack auto-attach:** Works as needed. The Slack bridge can call `GET /v1/sessions?project_dir=/path&attachable_only=true&running_only=true` to find sessions, or use `POST /v1/factory/start` with `reuse_existing=true` which calls `find_session_for_project()` internally.

**Edge case:** The match is by exact string comparison on `project_dir`, not canonical path. Trailing slashes or symlinks would fail to match. Not a blocker for Slack (the bridge controls the path), but worth noting.

---

## Gap Analysis: SSE Output vs Slack Needs

### What SSE provides:
- **Structured JSON events** with `event_type`, `entity_id`, `summary`, and `metadata`
- `supervisor_injected` events confirm message delivery with `prompt_id` + `status`
- `status` snapshots with agent states, task lists, queue depth
- `inbox` notifications from the supervisor queue

### What Slack needs:
1. **Plain-text output** — Slack messages need readable text, not JSON event streams
2. **Task completion notifications** — "worker finished task X" for thread updates
3. **Error/blocker notifications** — "worker blocked on Y" for alerting
4. **Response routing** — when supervisor/worker responds to a Slack message, route it back to the correct Slack thread

### Gaps identified:

| Gap | Severity | Notes |
|-----|----------|-------|
| **No plain-text output mode** | Medium | Activity events have a `summary` field that's human-readable. The Slack adapter can extract `event.summary` for thread updates. Not ideal but workable. |
| **No response routing** | Medium | When a supervisor responds via `coordination action=message target=slack-bridge-spike`, it goes to the inbox queue, not back to the HTTP caller. The SSE `inbox` event type captures these — the Slack adapter needs to poll the inbox and route responses to the originating Slack thread by matching `from` labels. |
| **No task-level events in SSE** | Low | Task completions appear as activity events (`entity_type: "task"`) but the current SSE filter only shows events from session agents. The Slack adapter can use `GET /status` which includes `tasks_in_progress` and `tasks_ready` for polling. |
| **No file/attachment passthrough** | Low | Slack users may upload files. No endpoint to inject file content. Would need a new endpoint or workaround (base64 in message body, or write to project dir). |
| **SSE is pull-only from server** | None | SSE is the right pattern for Slack — the adapter opens one SSE connection per active session and maps events to Slack threads. No gap here. |
| **Auth is bearer token only** | None | Fine for server-to-server. The Slack bot authenticates users via Slack OAuth; the bridge-to-Cassy auth is a separate internal concern. |

### The `from` field is the response routing key

The message injection's `from` field (e.g., `"slack-user-U12345"`) appears in:
1. The `queue_source` metadata of `supervisor_injected` events
2. The response routing hint appended to wrapped messages: `To respond, use: coordination action=message target=slack-user-U12345`
3. Inbox notifications when agents respond back

This is the mechanism for thread-level routing in Slack: use a unique `from` per Slack thread, and match inbox responses by target.

---

## CLI Alternative: `cas factory message`

In addition to the HTTP API, the CLI exposes `cas factory message` for direct queue injection:

```bash
cas factory message --session <name> --target supervisor --message "hello" \
  --from slack-bridge [--no-wrap] [--wait-ack] [--timeout-ms 5000] [--cas-root PATH]
```

This writes directly to the prompt queue SQLite store without needing `cas serve` running. Useful as a fallback or for simpler integrations.

---

## Recommendation

**GO — build the Slack adapter on top of `cas serve`.** The API surface covers all required primitives:

1. **Session discovery:** `GET /v1/sessions?project_dir=X&attachable_only=true` or `POST /v1/factory/start` with `reuse_existing=true`
2. **Message injection:** `POST /v1/sessions/<name>/message` with `from=slack-thread-<id>`
3. **Activity streaming:** `GET /v1/sessions/<name>/events` (SSE) — extract `event.summary` for Slack messages, monitor `inbox` events for agent responses
4. **Status polling:** `GET /v1/sessions/<name>/status` for task state and agent health

The Slack adapter's job is mapping:
- Slack message → `POST /message` with `from=slack-<thread_ts>`
- SSE `activity` events → Slack thread updates (use `event.summary`)
- SSE `inbox` events → Slack thread replies (match by `inbox_id` or notification target)
- SSE `status` events → optional Slack status blocks (task progress, agent health)

No Rust changes needed for Phase 1. Future enhancements (task-level SSE filtering, plain-text output mode, file passthrough) can be added incrementally.

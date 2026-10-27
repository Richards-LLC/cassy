# Commander work events (schema v1)

Commander can show what a factory session is doing between its replies: the
supervisor's thinking, its tool calls, todo updates, task transitions and
worker events. The factory daemon serves these over the existing hub
connection as typed **work events**.

## Zero model tokens

Work events are read from data that already exists on the machine:

| Source | Events |
| --- | --- |
| The supervisor's Claude Code transcript (JSONL) | `thinking` (non-empty thinking blocks as recorded), `tool` (each `tool_use`, paired with its `tool_result`), `todo` (`TodoWrite`) |
| The supervisor's Codex rollout | `thinking` (reasoning summaries), `tool` (`function_call`, `custom_tool_call`, `local_shell_call`, paired with their outputs), `todo` (`update_plan`) |
| The supervisor queue in the task DB | `task` (started, blocked, parked for merge, close rejected, closed, reopened; merge-queue and verification notices), `worker` (died, idle, stalled), `notice` (other factory notices) |
| Hook events in the task DB | `worker` (commits, refused pushes, verification blocks, subagents) |

The daemon never calls a model, never writes to a transcript or an agent, and
never reads a credential file. The `claude` and `codex` binaries run exactly
as before, so the stream being on or off changes no agent's token count.
Claude Code records most thinking as an empty signed block; those blocks have
nothing to show and are skipped.

The first read of a transcript indexes its last 64 MiB; later reads index only
what was appended. Older turns are not served. The work runs on the daemon's
blocking job lane, never on its event loop.

## Protocol

The daemon advertises the `work_events` capability in `Welcome`. A client sends
the requests below only when that capability is present. Both need the
`pane:read` scope. The hub routes each answer only to the socket that asked,
the same as conversation history, so a client that never asks never receives a
work-event frame.

```json
{"WorkEventsRequest": {"request_id": "events-1", "before": null, "limit": 100}}
{"WorkEventDetailRequest": {"request_id": "detail-1", "event_id": "t:<agent>:000000000001234:001"}}
```

A page holds at most 100 events. `limit` 0 means 100. The page holds the newest
events before `before`, listed oldest first. While `has_earlier` is true, pass
`next_before` back as `before` to page further back. The cursor is opaque.

```json
{"WorkEvents": {
  "request_id": "events-1",
  "schema_version": 1,
  "events": [
    {"id": "t:sup-1:000000000001234:001", "kind": "tool", "session": "factory-1",
     "agent": "calm-otter-1", "turn": "prompt-1", "at": "2026-10-27T12:00:02+00:00",
     "label": "Bash: Check the crate", "has_detail": true, "error": true}
  ],
  "has_earlier": true,
  "next_before": "1793102402000:t:sup-1:000000000001234:001"
}}
{"WorkEventDetail": {"request_id": "detail-1", "event_id": "t:sup-1:000000000001234:001",
  "detail": "Bash\n\nInput:\n...\n\nResult (error):\n...", "truncated": false}}
```

| Field | Meaning |
| --- | --- |
| `id` | Stable identity; the key for `WorkEventDetailRequest`. |
| `kind` | `thinking`, `tool`, `todo`, `task`, `worker` or `notice`. Clients should treat an unknown kind as `notice`. |
| `session` | The factory session. |
| `agent` | The agent name that produced it, when known. |
| `turn` | The transcript turn it happened in: a Claude prompt id or a Codex turn id. |
| `at` | RFC 3339 time. |
| `label` | One line, at most 120 characters, redacted. |
| `has_detail` | True when a detail request has more to show. |
| `error` | Present and true when the tool call failed. |

The detail is the full thinking text, a tool's input and result, or a task-DB
row's payload. It is sent only when requested and is cut at 16 KiB, with
`truncated` set. `detail` is null when the event no longer resolves: the
transcript was rotated, the row was pruned, or the event belongs to another
session. A failed request is answered with `Error` whose `client_ref` is the
`request_id`.

## Redaction

Labels and details pass through Cassy's existing redaction before they leave
the daemon:

- Lines holding a secret assignment, a bearer credential or a PEM block are replaced.
- Secret-shaped words, such as `sk-`, `ghp_`, `github_pat_`, `AKIA` and `Bearer`, are replaced.
- Secret-named JSON fields in task-DB payloads are replaced.

## Versioning

Additions that older clients can ignore, such as a new optional field or a new
kind, keep `schema_version` 1. A change to an existing field's meaning raises it.

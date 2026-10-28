# Commander work events (schema v2)

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

`agent` is optional. When set to an agent's name, the page is that agent's own
timeline: its transcript, its task-DB rows and its hook events. A worker's
transcript is read only for such a request. Send it only to a daemon that
advertises `work_events_live`.

A page holds at most 100 events. `limit` 0 means 100. The page holds the newest
events before `before`, listed oldest first. While `has_earlier` is true, pass
`next_before` back as `before` to page further back. The cursor is opaque.

```json
{"WorkEvents": {
  "request_id": "events-1",
  "schema_version": 2,
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

## Change notices

A daemon that advertises `work_events_live` tells each client that has read a
page when the stream gains events:

```json
{"WorkEventsAppended": {"schema_version": 2}}
```

The notice carries no event content. The client then reads the newest page
again. The daemon checks for changes on its blocking job lane at most every
2 seconds, and only while such a client is connected. It compares the session
stream's newest event and the length of each worker's transcript; it stats a
worker transcript and does not read it. The hub relays the notice only to
sockets that have read a work-events page.

## Redaction

Labels and details pass through Cassy's existing redaction before they leave
the daemon:

- Lines holding a secret assignment, a bearer credential or a PEM block are replaced.
- Secret-shaped words, such as `sk-`, `ghp_`, `github_pat_`, `AKIA` and `Bearer`, are replaced.
- Secret-named JSON fields in task-DB payloads are replaced.

## Versioning

Version 2 is additive: all v1 fields, IDs, kinds and detail requests retain their
meaning. Old daemons omit extensions and remain readable. Old clients ignore
extensions; unknown kinds still read as notices. Unknown tool operations,
lifecycle stages, outcomes and provenance strings must not reject a base row.
The page and change notice advertise `schema_version: 2`; clients validate each
optional family independently instead of refusing unfamiliar versions.

## Optional v2 facts

| Field | Shape and meaning |
| --- | --- |
| `tool` | `{name, operation, targets, command?, cwd?, exit_code?, counts?: {files?, matches?}}`. Operations are `read`, `search`, `command`, `edit`, or `other`. Unknown tools retain their name, label and detail. Targets come from explicit tool arguments or patch headers, never shell-command guesses. Missing counts/exit codes mean unknown. |
| `lifecycle` | `{id, stage, phase?, outcome?, started_at?, ended_at?}`. A tool row starts at `start`, becomes `phase` while a command session is running, and becomes `end` on its matching final output. ID and row timestamp remain stable. Outcomes include `success`, `error`, `unknown`. Turn/compaction records become notice rows with lifecycle metadata. |
| `reasoning` | `{summary, source}`. Source is `codex_summary` or `claude_thinking`; summary is the first published nonempty line. The existing on-demand `detail` contains only the recorded published text. Encrypted content, signatures, analysis messages and unrecorded reasoning are never interpreted or returned. |
| `changed_files` | `{files: [{path, operation, source_event_id}], provenance: "successful_tool_calls", complete: false}`. A cumulative snapshot of successful explicit edit targets for this turn. Use the newest snapshot per agent/turn. It proves tool results, not a Git diff or a commit. Shell side effects and other agents' edits are unknown. |

```json
{"id":"t:agent:000000000001234:001","kind":"tool","session":"factory-1",
 "at":"2026-10-28T12:00:00Z","label":"Read: src/lib.rs","has_detail":true,
 "tool":{"name":"Read","operation":"read","targets":["src/lib.rs"],"counts":{"files":1}},
 "lifecycle":{"id":"call-1","stage":"end","outcome":"success",
 "started_at":"2026-10-28T12:00:00Z","ended_at":"2026-10-28T12:00:01Z"}}
```

The source adapters are in `daemon/runtime/work_events.rs`. Claude publishes
`assistant.message.content[].tool_use` and `user` tool results, and system
`turn_duration`/`compact_boundary` records where available. Codex publishes
`response_item` calls, outputs and `reasoning.summary`, and `event_msg`
`task_started`/`task_complete` records. Codex command outputs with a session ID
are still running; a later `write_stdin` result can finish that operation.
Claude background Bash results (`isAsync`/`backgroundTaskId`) similarly stay
running until `TaskOutput` returns a completed task. Continuation rows carry the
original tool scope and lifecycle ID, so clients can group by `lifecycle.id`
even after the initial call has paged out. A Claude `turn_duration` record
reports an end with unknown outcome; it does not prove the turn succeeded.
Compaction boundary records publish the completed `compacted` phase, not an
invented in-progress interval. Opaque wrapper tools remain `other`: the adapter
does not interpret JavaScript or shell scripts to guess their inner operations.
No adapter invents setup, approval waiting, completion or file changes from
prose. Some harness versions do not publish every phase; absence is unknown.

All new text passes the same redactor as labels/details. Lists and strings are
bounded. The hub validates correlation using the shared protocol and forwards
the original frame bytes, preserving optional fields through mixed versions.
Change probes include transcript lengths so completion of an existing row also
triggers a refresh; clients merge repeated IDs to replace their earlier state.

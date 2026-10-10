# Trace archives

Cassy keeps the raw event and terminal-recording layer available after the
30-day live-retention window. During daemon maintenance, old rows are first
written as new zstd-compressed JSONL files under `.cas/archive/`; only after a
successful write are the live rows removed. Archive files are write-once and
are never opened for append or update, so each file is a stable sampling unit.

The archive directory is bounded by compressed bytes, not by an age or
existence window. Configure the finite cap in `.cas/config.toml`:

```toml
[daemon]
archive_max_bytes = 1073741824 # 1 GiB (the default)
```

When the cap is exceeded, maintenance removes the oldest archive files first
and emits a `trace archive` eviction log line for every file removed. A cap of
zero is rejected; this prevents an accidental unlimited archive. The legacy
`daemon.archive_retention_days` key is still accepted for config-file
compatibility, but it no longer controls archive retention.

The storage API provides `list_archived_traces` for an inclusive timestamp
range and `sample_archived_traces` for a deterministic, evenly-spread sample
of that range. Event timestamps use `created_at`; recording timestamps use the
record's `created_at`. The decoded result keeps its source archive path so a
maintainer can inspect or stratify samples by archive file without restoring
data into the mutable live database.

## Upgrade note

This change is forward-only. Events or recordings that an older Cassy version
already hard-deleted cannot be reconstructed by an upgrade. Once the new
daemon runs, rows that cross the 30-day live-retention boundary are archived
before removal. Existing live rows remain available until their next
maintenance cycle, and the configured byte cap applies to newly-created and
pre-existing `.jsonl.zst` archive files alike.

## Telemetry event retention

High-volume telemetry events are deleted rather than archived once they are
older than `factory.event_telemetry_retention_days`, which defaults to 14 days.
These are `supervisor_injected`, `supervisor_notified`, `agent_heartbeat`,
`worker_file_edited`, `worker_subagent_spawned` and `worker_subagent_completed`.
Every reader of these types looks only at a recent window, such as injection
acks, worker status, the activity feed or the director.

```toml
[factory]
event_telemetry_retention_days = 14 # 0 keeps every telemetry event
```

The canonical `cas serve` daemon (the `daemon.sock` election winner) runs this
pass every 15 minutes, whether or not the project is idle. Each delete is a
`BEGIN IMMEDIATE` transaction of at most 1,000 rows, with the connection
released for 20 ms between batches. One pass runs at most 500 batches, and any
remaining backlog drains on later passes. `cas daemon` maintenance runs the
same pass.

Lifecycle events are never removed by this window. These include task,
commit, verification, worker-death and push-block events. Commit provenance
and task-ownership inference read `worker_git_commit` and the first task event
per task across all time. Those rows leave the live table only through the
30-day archive above, which runs only when maintenance has `auto_prune`
enabled. The embedded daemon keeps `auto_prune` disabled.

The queue loop writes one `supervisor_injected` row per distinct outcome. An
identical retry, meaning the same prompt, recipient, status and error, is
skipped. On the cassy store this would have kept 39,254 of 799,578 rows.

## Prompt and queue table retention

The same canonical-daemon tick bounds three more tables. Each pass uses
`BEGIN IMMEDIATE` transactions of at most 1,000 rows, with the connection
released for 20 ms between batches. Each table gets at most 200 batches per
pass, and any remaining backlog drains on later passes. Setting a window to 0
turns off its pass.

```toml
[factory]
prompt_transcript_retention_days = 14 # clear prompts.messages_json on older prompts
prompt_retention_days = 7             # delete terminal prompt_queue rows
supervisor_queue_retention_days = 14  # delete finished supervisor_queue rows
```

- **`prompts`**: the session transcript (`messages_json`) is cleared on older
  prompts. The row itself is kept with its text, session, agent, task and
  content hash, so `blame` snippets, attribution (`file_changes.prompt_id`)
  and the carried-forward prompt still resolve. No reader consumes the
  transcript, and a cleared value reads back as an empty message list.
- **`prompt_queue`**: terminal rows are deleted together with their
  per-recipient seen and transport receipts. Pending rows are never deleted.
  Rows held by the operator delivery outbox are never deleted. Rows that carry
  a relay episode key, such as `ci-red-run:`, are never deleted, with one
  exception. A supervisor-queue outbox key (`lifecycle-outbox:`,
  `worker-died-outbox:` or `worker-attention-outbox:`) is deleted once its
  notification has been delivered or no longer exists. The outbox re-sends
  only notifications that have not been delivered, and notification ids are
  never reused. Deleted rows drop out of Commander thread history and
  `message_status`.
- **`supervisor_queue`**: an outbox notification is deleted once its prompt has
  been delivered. A pulled notification is deleted once it has been processed.
  Pending and undelivered notifications are kept. Keys that can recur for the
  same subject (`worker-attention:` and `integration:`) are also kept, so
  `notify_idempotent` cannot fire the same event a second time.

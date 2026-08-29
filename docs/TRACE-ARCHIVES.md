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

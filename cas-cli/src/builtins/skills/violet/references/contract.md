# Violet tool contract (client summary)

This is the Cassy-side summary of the hub's `docs/TOOL_CONTRACT.md` (violet_ps,
2026-10-09). The hub document wins on any disagreement. Every call answers
`{"ok": true, "schema_version": 1, …}` or
`{"ok": false, "error": {"code", "message", "retryable"}}`, with optional
`retry_after_ms` and a safe `error.upstream` object. There is no `ts` field.

## `violet_read`

`channel` is the only required input; unknown fields are rejected.

| Field | Default / limit |
| --- | --- |
| `since` | ISO 8601 with offset; includes messages at or after it |
| `max_messages` | 200; 1–500 |
| `mentions_only` | false |
| `include_threads` | true |
| `include_recent_replies_to_older_roots` | true; with `since`, also expands older roots whose latest reply is in the window |
| `thread_lookback_days` | 7; 1–30, measured before `since` |
| `include_files` | true |
| `include_channels` | true for compatibility; pass `false` on every read except directory discovery |
| `file_id` / `file_ids` | only these files (array ≤ 50); history, enrichment and `messages` are skipped |
| `message_id` | exactly one root or reply |
| `thread_id` | the root and every reply, independent of `since` and `include_threads` |
| `max_files` | 20; 0–50 |
| `max_file_bytes` | 1048576; up to 4194304 |
| `max_bytes` | 100000; 65536–8388608 |
| `cursor` | opaque continuation from a partial read; not combinable with `file_id`, `message_id` or `thread_id` |

**Byte budget.** The default 100000 UTF-8 bytes measures the serialized MCP
tool result, including its text wrapper and JSON escaping. Interactive clients
keep the default; an explicit `max_bytes` up to 8 MiB is for raw-HTTP scripts
that verify larger files. A metadata-only envelope over budget still fails
`size_cap_exceeded`: narrow `since`, lower `max_messages`, or scope the read.

**Channel directory.** `channels[]` (`{id, name, is_private}` for every bot
channel) appears only while `include_channels` is true. Do not depend on it:
request it only to discover channels, and pass `include_channels: false` for
bounded reads, dedupe reads and file verification. `channel` is always the
resolved request.

**Receipt.** `channel`, optional `channels`, `messages[]`, `files[]`,
`skipped_files`, `counts {messages, roots, threads_expanded, files,
downloaded_bytes}`, `filters` (including any `file_ids`, `message_id`,
`thread_id`), `complete`, optional `cursor`, optional `threads_failed[]`
(`{thread_id, code, retryable, upstream?}`) and optional `thread_roots[]`
(200-character context for older roots). A message carries `message_id`,
`thread_id`, `parent_message_id`, `created_at`, `author`, `text`, `permalink`,
`file_ids` and `reactions[] {name, count, users}`.

**Partial reads.** `complete: true` covers message retrieval only. A read that
hits the 50-page budget returns `complete: false` and a `cursor`: resend the
same channel, filters and limits with that cursor, unchanged, and concatenate
the slices. A failed thread keeps its root, lists itself in `threads_failed`,
and lets the others expand; re-read it with `thread_id`.

**Skipped files.** A file over `max_file_bytes` or the aggregate budget keeps
its metadata, omits `content_base64`, and sets `skipped: true` with
`skip_reason` `exceeds_max_file_bytes` or `exceeds_max_bytes`. The hub streams
skipped files up to 16 MiB (16777216 bytes) to fill `sha256`; above that cap,
or when the stream fails, `skip_reason` is `hash_unavailable` and `sha256` is
absent. `skipped_files` counts them. To verify an upload, read its `file_id`
and compare `size_bytes` and a present `sha256` with the local file; without a
hash, report integrity as unverified. Request a larger explicit `max_bytes`
only when the decoded bytes are needed too.

## `violet_post`

The served schema is flat: `channel` and `kind` are required, and `kind` is one
of `message`, `file`, `file_external`, `reaction`, `edit`, `delete`, `thread`.
Fields from another kind are rejected with a message naming the right one.
Every kind accepts `deliverable`, `task_id`, `epic_id` and `operator_override`
as Cassy metadata the hub ignores (the Cassy publication gate reads them).

| Kind | Fields |
| --- | --- |
| `message` | `text`, `reply_to?` |
| `file` | `file {filename, content, content_encoding?, content_type?, sha256?, size_bytes?, title?}` or `files[]` of the same (1–10), `initial_comment?`, `reply_to?` |
| `file_external` | `step` (`begin`/`complete`), `filename`, `size_bytes`, `sha256`, `title?`, `initial_comment?`, `reply_to?`, and `file_id` on `complete`; or `files[]` (1–10) of that metadata |
| `thread` | `text`, `idempotency_key` (1–200 chars), `replies[]` (1–20) of `{text?, files?}` |
| `reaction` | `message_id`, `reaction`, `action` (`add`/`remove`) |
| `edit` | `message_id`, `text` (Violet-authored messages only) |
| `delete` | `message_id` (Violet-authored messages only) |

`file` and each reply are nested objects; every other field is top level, for
example `{"channel":"cas-scratch","kind":"edit","message_id":"1710000000.000002","text":"Updated"}`.

### File content

- Text files (md, csv, html, json, txt): `content` is the plain text and
  `content_encoding` is `"text"`, the default. No base64.
- Binary files (png, jpg, pdf, zip): `content` is base64 and
  `content_encoding` is `"base64"`. A script reads and encodes the bytes from
  a path; they are never pasted through the model.
- Inline limit: 1048576 decoded bytes per call, aggregated across `files[]`
  and across every reply of a `thread`. Over it the hub answers
  `file_too_large`; switch to `file_external`, which carries no bytes through
  MCP. External uploads allow up to 1 GB per file.
- Declared `sha256` and `size_bytes` are checked before any Slack write
  (`file_integrity_mismatch`); a `.png` or `image/png` is structure-checked.

### External upload

1. `step: "begin"` with `filename`, `size_bytes`, `sha256` (and any title,
   comment or `reply_to`) returns `upload_url`, `file_id` and `expires_at`.
2. Stream the raw bytes from disk to `upload_url`, for example
   `curl --data-binary @<path> <upload_url>`. Send no hub credentials there.
3. `step: "complete"` with the same metadata plus `file_id` returns a `file`
   receipt. An expired or unknown upload is `file_upload_unavailable`: begin
   again rather than retrying completion.

### Ordered thread

`kind: "thread"` posts the root and every reply in one call, paced at one
second, and answers `{kind: "thread", idempotency_key, posted: [{index,
message_id, thread_id, permalink, files?}], failed_index, resume_safe}`. Index 0
is the root. A reply holds inline files or external metadata
(`{file_id, filename, size_bytes, sha256, title?}`), never both. Resending
identical content with the same key returns the stored receipts without
reposting; different content on a used key is `invalid_input`. After a failure,
retry the identical request only when `resume_safe` is true; otherwise read
the thread with `thread_id` before doing anything else. `ambiguous_post` means
another call with that key may still finish. Never change the key to get past a
stopped record. A hub without the thread journal answers
`capability_unavailable`; post the parent and replies one by one with
`reply_to`.

### Receipts

```json
{"ok": true, "schema_version": 1, "kind": "message",
 "channel": {"id": "C0…", "name": "cas-internal"},
 "message": {"message_id": "…", "thread_id": "…", "permalink": "https://…"}}
```

A top-level message has `thread_id` equal to its own `message_id`; a reply
carries its parent's `message_id`. Treat `permalink` as opaque. File receipts
add `file {file_id, name, size_bytes, sha256, sha256_verified, state,
image_validated, permalink}` (or `files[]` for a batch); `state` is `attached`
or `uploaded`, and `sha256_verified` is true only when the hub re-hashed
Slack's copy (files up to 16 MiB). A failed completion can still have shared a
file: inspect Slack before replaying. Reaction receipts add
`reaction {name, action, changed}`; the bot removes only its own reactions, and
`reaction_not_owned` names the case where the reaction belongs to someone else.

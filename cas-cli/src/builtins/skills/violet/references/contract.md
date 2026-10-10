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

## Posting local files with `cas violet`

Never put file content in a `violet_post` call. From any repository, give the
command a path and it reads the bytes from disk:

```text
cas violet post   --channel <name> [--text "…"] [--reply-to <message_id>] [--file <path>]... [--json]
cas violet thread --channel <name> --text "…" --reply "…" [--reply-file <path>]... [--reply "…"]... --idempotency-key <key> [--json]
cas violet read   --channel <name> --since <RFC3339> | --thread <message_id> | --message <message_id> [--cursor <c>] [--max-messages N] [--json]
```

- **post**: with no `--file`, a `kind: "message"`. With files (1–10), `--text`
  becomes `initial_comment`. Each file is sent as `content_encoding: "text"`
  when its bytes are UTF-8 without binary control characters (and carry no
  PDF, PNG, JPEG, ZIP or GIF signature), otherwise as base64, with its
  `size_bytes` and `sha256` declared. Up to 1048576 bytes in total go inline in
  one call (`file` or `files[]`); anything larger uses `file_external`: begin,
  each file streamed from disk to its `upload_url` (which is never printed),
  then complete.
- **thread**: one `kind: "thread"` call. Replies keep the order given; each
  `--reply-file` attaches to the `--reply` before it. Files across the whole
  thread go inline up to 1048576 bytes; above that every file is uploaded
  first and its reply carries the external metadata. A failure keeps
  `posted[]`, `failed_index` and `resume_safe`; follow the ordered-thread rules
  above.
- **read**: `violet_read` with `include_channels: false`. A channel read
  requires `--since`. Downloaded file bytes are dropped from the output
  (`content_omitted: true`); sizes and hashes stay.

Human output is a verdict line, the message and file receipts (`message_id`,
permalink, file ids, `sha256_verified`) and a hint. `--json` prints the hub's
receipt, or its error receipt, as one document. Every `ok: false` exits
non-zero, and the hub's own codes (`not_member`, `file_too_large`,
`file_integrity_mismatch`, …) pass through unchanged. The command's own codes:

| Code | Meaning |
| --- | --- |
| `invalid_input` | a flag is missing or out of range, a path is empty or not a file, or more than 10 files per message or 20 replies |
| `local_request_failed` | a file could not be read, or changed while being posted |
| `not_configured` | no `[servers.violet]` registration here; run `cas integrate violet` |
| `missing_credential` | a credential variable named by the registration is unset (named, never printed) |
| `invalid_token` | HTTP 401: the hub rejected the bearer (or the Vercel bypass); run `cas integrate violet`, then `cas doctor` |
| `hub_unreachable` | the hub did not answer; a write may be unconfirmed, inspect before retrying |
| `hub_bad_response` | the hub's answer is not a Violet receipt; a write may be unconfirmed |
| `denied_by_policy` | the proxy allowlist or worker policy does not admit the route |
| `publication_blocked` | the post shares a file while its epic's verification is open ([publication](publication.md)) |
| `violet_upload_failed` | the direct upload was refused; nothing was shared |
| `violet_post_failed` | the hub refused without a code of its own |

An error that lists `uploads_begun` means Slack allocated those uploads: check
the channel before posting again. `cas doctor` shows readiness on its Violet
row: hub reachable and bearer accepted, or `invalid_token` for a bad bearer.

## Posting a published artifact by ID

To share a file that `artifact action=publish` already committed to Cloud,
call the Cassy `artifact` tool instead of building a `violet_post`:

```text
artifact action=post id=<art-…> channel=<name or id> [reply_to=<message_id>] [title=<text>] [initial_comment=<text>]
```

Cassy resolves the record through Cloud with this installation's Cloud login,
downloads it, and refuses bytes whose size or SHA-256 differ from the published
record (records are 1 byte to 25 MiB). It then runs Violet's `file_external`
begin, direct upload and complete steps through the registered proxy. The caller
never handles bytes, signed URLs or upload URLs.

The receipt lists `artifact_id`, `channel_id`, `message_id`, `file_id`,
`permalink`, `size_bytes`, `sha256` and `sha256_verified`, and the permalink is
saved on the artifact record. `sha256_verified: false` means Violet did not
hash the Slack copy, either because the file is over 16 MiB or because the hub
reported no hash. Check it with `violet_read` `file_id` before calling it
verified.

| Code | Meaning |
| --- | --- |
| `artifact_id_missing` / `artifact_channel_missing` | `id` or `channel` was not given |
| `artifact_not_found` | no published artifact by that ID in this project or Cloud account |
| `artifact_not_in_cloud` | the record is not committed to Cloud; publish it again while logged in |
| `cloud_auth_missing` | not logged in to Cassy Cloud; run `cas cloud login` |
| `cloud_resolve_failed` / `artifact_store_failed` | Cloud or the local artifact store could not be read |
| `artifact_size_invalid` | the record's size is outside the publishable range |
| `artifact_size_mismatch` / `artifact_checksum_mismatch` | the downloaded bytes differ from the record; nothing was posted |
| `artifact_download_failed` | the artifact bytes could not be fetched |
| `violet_unavailable` | no proxy is configured or the Violet call failed; run `cas integrate violet` |
| `violet_upload_failed` | the direct upload was refused; nothing was shared |
| `violet_invalid_receipt` | Violet's receipt is unreadable or names another file |
| `slack_integrity_mismatch` | Violet reports a Slack copy whose size or SHA-256 differs |
| `violet_post_failed` | Violet refused without a code of its own |

Violet's own codes, such as `file_integrity_mismatch`, pass through unchanged.
An error that carries a `file_id` means Slack allocated an upload: check the
channel before posting again. The publication gate treats `artifact
action=post` as a `kind: "file"` share, so it is refused while the artifact's
epic still has open verification.

# Violet tool contract

This document describes the public MCP surface at `/mcp/slack`. Authenticated
`tools/list` returns exactly these tools, in this order:

```text
["violet_read", "violet_post"]
```

The retired transition tool aliases are no longer registered. Use the two
Violet names above.

## Versioning

The current `schema_version` is `1` on successful read and post receipts under
either tool. The version changes only for a breaking contract change:
a field is removed, renamed, or retyped, or a tool is renamed. Additive fields
ship under the current version with a dated changelog entry. Retiring the transition aliases does not change the canonical tools or their
receipt fields, which remain at `schema_version: 1`.

## `violet_read`

Supply `channel` for a normal read, or `external_ref` to resolve a specific
Slack thread. Channel names may have one leading `#`.
Unknown input fields are rejected.

| Field | Type | Default / limit |
| --- | --- | --- |
| `channel` | string | Required unless `external_ref` is present; non-empty after trimming |
| `external_ref` | Slack permalink string or object | Optional; `{workspace_id?, channel_id, root_ts}` or `{workspace_id?, channel_id, message_ts}`; exactly one timestamp, as a string with six fractional digits |
| `cursor` | opaque string | Optional; continuation returned by a partial read |
| `since` | ISO 8601 datetime with offset | Optional; includes messages/replies at or after this time |
| `mentions_only` | boolean | `false` |
| `include_threads` | boolean | `true` |
| `include_recent_replies_to_older_roots` | boolean | `true`; when `since` and `include_threads:true`, also discover recent replies on older roots |
| `thread_lookback_days` | integer | `7`; minimum `1`, maximum `30`, measured before `since` |
| `include_files` | boolean | `true` |
| `include_channels` | boolean | `true` (compatibility); use `false` for lean reads |
| `file_id` / `file_ids` | string / string array | Optional; only these files, deduplicated; array maximum `50` |
| `message_id` | Slack timestamp string | Optional; exactly one root or reply |
| `thread_id` | Slack root timestamp string | Optional; root + all replies, independent of `since` and `include_threads` |
| `max_messages` | integer | `200`; minimum `1`, maximum `500`. Caps a complete digest of the window; it does not select the latest messages (use `latest`) |
| `latest` | integer | Optional; `1`–`200`. Channel-history reads only: return the newest `latest` root messages of the window (with their in-window replies); thread replies that appear in history do not count toward it. When older messages remain, `complete:false` and a `cursor` continue with the next-older slice; send the cursor with the same arguments. Cannot be combined with targeted reads |
| `max_files` | integer | `20`; minimum `0`, maximum `50` |
| `max_file_bytes` | integer | `1048576`; minimum `1`, maximum `4194304` |
| `max_bytes` | integer | `100000`; minimum `65536`, maximum `8388608` |

`external_ref` resolves through this hub's authenticated Slack installation.
The hub derives `workspace_id` from `auth.test.team_id`, cached for the client
instance; caller-provided workspace names and IDs never establish authority.
Permalinks accept HTTPS Slack `/archives/<channel>/p<timestamp>` URLs (also
decimal timestamps and a trailing slash), including workspace subdomain aliases
and reply links with `thread_ts` and `cid` query parameters. URLs are parsed,
never fetched. The bot must be a current member of the referenced channel.
A supplied `workspace_id` (or permalink `team` parameter) that differs from
the installation returns `foreign_workspace`; unproven membership returns
`not_member`. Other providers, conflicting query parameters and unverified
thread roots return `invalid_input`.

Root and reply references converge on the verified root, including replies
whose permalink lacks `thread_ts`. Slack history/replies verify both the
referenced message and its root; a supplied `thread_ts` is only a lookup hint.
Reply discovery shares a bound of 50 history/replies pages and returns
`pagination_exhausted` when that bound prevents verification.

An `external_ref` read adds
`identity: {workspace_id, channel_id, root_thread_ts}` to the receipt and, by
default, returns the complete targeted thread with the existing digest fields.
Use `include_threads:false` for identity resolution alone:

```json
{
  "ok": true,
  "schema_version": 1,
  "identity": {
    "workspace_id": "T123",
    "channel_id": "C123",
    "root_thread_ts": "1710000000.000001"
  }
}
```

`external_ref` cannot be combined with `since`, `cursor`, `latest`, `message_id`,
`thread_id`, `file_id`, `file_ids` or `mentions_only:true`. If `channel` is also
supplied, it must resolve to the same channel. Identity-only resolution skips
content, attachment and channel-directory retrieval.

Every returned message, `thread_roots` context item and `threads_failed` item
now carries additive `workspace_id`, `channel_id`, and `root_thread_ts` fields.
They identify that item's thread, so a normal channel digest may contain
several root identities. Existing message/thread IDs remain present, timestamps
remain strings, and identity fields count toward the response byte budget.
These additions retain `schema_version: 1` under the versioning policy above.

With a `since` boundary and threads enabled, Violet scans roots as far back as
`since − thread_lookback_days` (default 7 days). It expands only older roots
whose `latest_reply` is at or after `since`; missing/malformed optional metadata
is checked conservatively. Replies are requested with Slack's `oldest` bound
and locally filtered at the exact timestamp. Roots outside the lookback are not
scanned. Set `include_recent_replies_to_older_roots:false` for the previous
root-only window, or use `thread_id` to retrieve a known thread regardless of
lookback. With `include_threads:false`, history remains bounded by `since`.

Older roots appear as minimal context in the additive `thread_roots` array:
`{workspace_id, channel_id, root_thread_ts, message_id, author: {id, name}, text, mentions, permalink}`. `text` contains at most 200
Unicode characters. The old root does not appear in `messages` or message/root
counts; only its replies at/after `since` do. Context is emitted with its first
returned reply and is not repeated on a cursor continuation. Ordinary in-window
roots retain their existing full message representation. The lookback and
expansion share the 50-page retrieval budget below, including re-fetching root
context on resume, and cursors bind both new lookback options.

The successful receipt has this shape:

```json
{
  "ok": true,
  "schema_version": 1,
  "channel": {"id": "C123", "name": "cas-internal", "is_private": true},
  "channels": [
    {"id": "C123", "name": "cas-internal", "is_private": true},
    {"id": "C0BUZEB4H3M", "name": "cas-scratch", "is_private": false}
  ],
  "messages": [],
  "files": [],
  "skipped_files": 0,
  "counts": {
    "messages": 0,
    "roots": 0,
    "threads_expanded": 0,
    "files": 0,
    "downloaded_bytes": 0
  },
  "filters": {
    "since": null,
    "mentions_only": false,
    "include_threads": true,
    "include_files": true
  },
  "complete": true
}
```

`channels` is omitted when `include_channels:false`; the default remains true while Cassy consumers migrate. When included, it contains every current public or private channel where the bot is a
member, with `{id, name, is_private}`. The designated consumer scratch channel
is `#cas-scratch` (Slack ID `C0BUZEB4H3M`, public). Directory discovery is
paginated and cached for five minutes. On a channel-name miss, a directory
at least 30 seconds old is refreshed once before returning `not_member`.
Concurrent misses share the refresh; subsequent misses within 30 seconds reuse
the directory. A successful read includes the refreshed `channels` list.
Membership is still proved before every operation. If the name remains absent,
the error says `Invite @Violet to #<name> and retry.` DM IDs return
`unsupported_channel` without calling Slack. Channel discovery failures with
Slack `missing_scope` or `not_allowed_token_type` return
`capability_unavailable`, `retryable:false`, and the safe upstream detail.

Each call spends at most **50 Slack history/replies page requests**, with at
most **200 messages per page** and three attempts per request for transport
failures or rate limits. History and expanded threads share this budget. A call
that reaches it returns the collected messages with `ok: true`, `complete:
false`, and an opaque `cursor`. Send that cursor with the same channel, filters,
and limits to continue; concatenate the slices. Earlier roots are retained
internally as thread context and are not repeated in subsequent slices.
The cursor is opaque base64url JSON containing a version, channel ID, a filter
fingerprint, and retrieval positions. It contains no message bodies, credentials,
or secrets and does not depend on the bot token. Membership is re-proven on
every call. Unknown versions, invalid positions, or mismatched channel/filters
return `invalid_input` with `retryable: false`. Keep the cursor unchanged.
A continuation within a history page re-fetches that page for thread context;
this counts toward the same 50-page budget. Pagination uses Slack's live
cursors; concurrent Slack changes can affect the snapshot, and a removed pending
root invalidates that continuation. Restart the read if Slack expires its own
cursor.
The output message/file/byte limits above still apply to each returned slice.
Cursors apply to channel history reads; targeted file/message/thread reads
cannot be combined with a cursor.

A failed thread keeps its root, omits its replies from the current slice, and
allows other threads to expand. Partial receipts include
`threads_failed: [{"thread_id": "1790784426.913449", "code": "slack_error",
"retryable": false, "upstream": {"error": "missing_scope"}}]`; `upstream` is
optional and contains only safe provider error/status/retry timing. Each slice lists failures encountered in that call. `complete` remains false
even after the last slice if any thread was skipped in an earlier slice. A partial read caused only by a
thread failure has no cursor when there is nothing left to retrieve. Replies
already returned in an earlier slice cannot be retracted if that thread fails
later. Successful complete receipts retain their existing schema-v1 shape.

Slack skin-tone reaction names such as `+1::skin-tone-2` are preserved. File-only
messages normalize to empty text; malformed optional reaction fields are
omitted. Unparseable message identity/timestamps produce non-retryable
`invalid_response` (per thread for replies, channel-level for history). Slack
permission/auth/contract rejections use non-retryable `slack_error`, with safe
`upstream` detail. Only identified transport failures, timeouts and Slack 5xx
outages use retryable `upstream_unavailable`. Repeated or malformed Slack
pagination cursors still use non-retryable `pagination_exhausted`.


Each message contains `message_id`, `thread_id`, `parent_message_id`,
`created_at`, `author`, `text`, `mentions`, `permalink`, `file_ids`, and `reactions`.
`mentions` is present on every root and reply as
`{user_ids:[], violet:false, broadcast:null}`. `user_ids` contains sorted,
deduplicated Slack user IDs parsed from `<@U…>` / `<@W…>` (including labelled
tokens) and rich_text `user` elements. `violet` is true when that list includes
Violet’s ID (`SLACK_BOT_USER_ID`, otherwise cached `auth.test` resolution).
`broadcast` is `"here"`, `"channel"`, `"everyone"`, or null, parsed from
`<!here>` / `<!channel>` / `<!everyone>` and rich_text broadcast elements.
If several broadcasts occur, the first in text wins, then the first in blocks.
No mention produces an empty list, false, and null. Mention metadata is included
in the serialized output byte budget; it is never dropped to fit a slice.
Minimal `thread_roots` contexts also retain mentions from the full root even
when the visible root text is shortened to 200 characters.

For responder selection ([Cassy #1146](https://github.com/Richards-LLC/cassy/issues/1146)),
a thread reply mentioning a human other than Violet, with `violet:false`, is
addressed to that human: Violet should wait for them, at most note it is waiting.
Starting the thread does not make Violet the requested reviewer. A Violet
mention or no user mention retains the existing reply decision.

Reaction entries contain `{name, count, users}`. `users` preserves the Slack
reactor user IDs, deduplicated and sorted, including for skin-tone reactions.
Slack may return fewer user IDs than the total `count`; the list contains only
IDs Slack supplied, and is empty when they are unavailable.
Each file contains `file_id`, `name`, `title`, `mime_type`, `size_bytes`, and
`permalink`. Downloaded files additionally contain `sha256` and
`content_base64`. When `size_bytes` exceeds `max_file_bytes`, its content is
omitted and the file contains `skipped: true` and
`skip_reason: "exceeds_max_file_bytes"`; it retains its metadata but omits
content. The hub optionally streams skipped files up to **16 MiB (16777216 bytes)**
to compute `sha256`. Above that cap or without a private download URL it makes
no download call. A skipped-file download, stream, or size-mismatch failure
also leaves the read usable: `skip_reason:"hash_unavailable"`, no `sha256`.
Skipped streams exceeding the cap at runtime are cancelled. This fallback
preserves the original metadata and does not affect other messages/files.
Permanent per-file read failures are isolated (#40). A refused `files.info`
returns that attachment with `skipped:true` and `skip_reason:"access_denied"`
or `"file_not_found"`. A refused private download, missing download URL,
invalid metadata, stream failure or size mismatch returns `"file_unreadable"`.
Messages and other readable attachments are retained. Unreadable entries omit
content and hashes, and may include `upstream:{error,status}` with only a safe
Slack code and/or HTTP status. They never include the private URL or response
body. Metadata learned from Slack is retained; when `files.info` supplied none,
`name`, `title`, `mime_type`, `size_bytes`, and `permalink` are null, while
`file_id` remains present. Null metadata is allowed only on these unreadable
skips; downloaded and size/hash-skipped file contracts stay the same.

A file-only scope (`file_id`/`file_ids`, without message/thread selection) with
all requested files unreadable returns non-retryable `file_unavailable`, with
an accurate message and safe upstream detail. A mixed file-only scope retains
readable content and unreadable metadata. Channel/message/thread scopes can
return all attachments skipped while retaining the messages. Slack rate limits,
5xx outages, and read capability/auth failures keep their existing read-level
error handling. Unknown-metadata skips and upstream detail count toward the
serialized byte budget and `skipped_files`; they add zero downloaded bytes.

`skipped_files` is the top-level count of skipped entries,
`counts.files` includes both downloaded and skipped entries, and
`counts.downloaded_bytes` excludes skipped bytes. Skipped files do not count
toward `max_files`; only files that would be downloaded consume that limit.
Counts and filters are always present as shown above. Optional scope inputs are
recorded as `filters.file_ids`, `filters.message_id`, and `filters.thread_id`.
A file-only scope bypasses history/enrichment and returns `messages:[]`. A
message scope can return a reply without its parent; parent/thread IDs retain
their original values. Combined scopes intersect message/thread selection,
and file selectors restrict downloaded attachments. Channel membership is
resolved before every read, including a file-only read.

The default **100000 UTF-8 byte** budget measures the serialized MCP tool result,
including its text wrapper and JSON escaping. JSON-RPC/SSE framing is outside
this budget. Explicit `max_bytes` up to 8 MiB is honoured for raw HTTP scripted
consumers verifying larger files; interactive clients should use the default.
Files that cannot fit the aggregate budget return metadata, optional streamed `sha256`,
`skipped:true`, and `skip_reason:"exceeds_max_bytes"`. Smaller files are admitted
first, with deterministic ID order for ties. Messages and other files survive;
`complete:true` means message retrieval completed, while `skipped_files` reports
omitted file content. A message/metadata-only envelope exceeding the budget
still returns `size_cap_exceeded`; narrow the scope or request fewer messages.
A digest-limit `size_cap_exceeded` carries a count-only `detail` naming the limit
that was hit, for example `167 messages in the read window exceed max_messages 1`
(a channel read without `since` or `latest` covers the whole retained history), `downloadable
files exceed max_files`, or the envelope byte size against its budget.
For upload verification, request `file_id` and compare `size_bytes` + a present `sha256`;
without a hash, report integrity as unverified;
request an explicit larger `max_bytes` when decoded bytes are also needed.

## `violet_post`

`violet_post` advertises a flat object schema with top-level `properties` and
`kind` as an enum, so MCP clients can discover every operation. `channel` and
`kind` are the only unconditionally required fields. Property descriptions
state the per-kind requirements, which the server validates strictly; fields
belonging to another kind and unknown fields are rejected. Validation errors
name the field and its expected value or type. For example, `kind: "message"`
with `message_id` directs the caller to `kind: "edit"` with `message_id` and
`text`.

Every kind accepts `deliverable` (boolean), `task_id`, `epic_id`, and
`operator_override` (strings) as optional Cassy metadata. The hub ignores
these fields: they do not alter Slack operations, receipts, or audit entries,
and they do not enable a hub-side verification gate.

From any repo, post local files with `violet post --channel CHANNEL --file PATH`
(or `violet thread --channel CHANNEL DRAFT_JSON`); never put file bytes in a tool
call. `violet` is `scripts/slack-post.sh`, linked onto PATH once by
`scripts/install-violet-cli.sh` (symlink only); the script can also be invoked by
absolute path. Its helpers resolve beside the real script, following symlinks;
a relative `PATH` resolves from the caller's working directory.
Text files (Markdown, CSV, HTML, JSON) go as plain UTF-8 text with
`content_encoding:"text"`; binary files (PNG, JPEG, PDF, ZIP) go as
`content_encoding:"base64"`. Encoding is chosen from the bytes, preserving them
exactly. Never hand-copy base64; give the helper a path. The single-file helper
automatically uses `file_external` for files over 1 MiB. Thread drafts use local
`path` entries relative to the draft JSON and retain multi-file support; when
aggregate inline bytes exceed 1 MiB, prepare `file_external` uploads and supply
the returned file metadata. Proxy-side local-path upload is tracked in
[Richards-LLC/cassy#1154](https://github.com/Richards-LLC/cassy/issues/1154).

Inline `kind: "file"` accepts decoded text or base64 content up
to `1048576` bytes. Larger files use the two-step `kind: "file_external"`
variant so the client can stream bytes directly to Slack without sending them
through the hub. The same inline limit applies to the aggregate decoded size
of `files[]`. Oversized inline uploads return `file_too_large` with observed
size, the `1048576`-byte limit, and instructions to use `file_external`.
The hub caps external uploads at `1000000000` bytes per file; workspace
restrictions may be lower. Slack documents a [1 GB file ceiling and up to 10
files per message](https://slack.com/help/articles/201330736-Add-files-to-Slack).
An external upload sends only metadata through MCP, so file bytes avoid the
hosting platform's JSON body limit. Raising the inline cap would not bypass
that platform limit; clients such as Cassy's release adapter must select the
external path before encoding a large file.

| Field / variant | Type | Default / limit |
| --- | --- | --- |
| `channel` | string | Required, non-empty after trimming |
| `kind` | enum | Required: `message`, `file`, `file_external`, `reaction`, `edit`, `delete`, or `thread` |
| `deliverable` | boolean | Optional on every kind; ignored Cassy metadata |
| `task_id`, `epic_id`, `operator_override` | strings | Optional on every kind; ignored Cassy metadata |
| `message.text` | string | Required, non-empty after trimming |
| `message.reply_to` | string | Optional, non-empty when present |
| `file.filename` | string | Required, non-empty after trimming |
| `file.content` | string | Required, non-empty; decoded content maximum `1048576` bytes |
| `file.content_encoding` | literal | `text` or `base64`; default `text` |
| `file.content_type` | string | Optional declared MIME type; `image/png` enables PNG validation |
| `file.sha256` | lowercase or uppercase hexadecimal string | Optional expected SHA-256 of decoded bytes |
| `file.size_bytes` | non-negative integer | Optional expected byte length of decoded content |
| `file.title` | string | Optional, non-empty when present |
| `initial_comment` | string | Optional, non-empty when present |
| `reply_to` (message/file) | string | Optional, non-empty when present |
| `file_external.step` | literal | Required: `begin` or `complete` |
| `file_external.filename` | string | Required for both steps; non-empty after trimming |
| `file_external.size_bytes` | positive integer | Required for both steps; declared byte length |
| `file_external.sha256` | lowercase or uppercase hexadecimal string | Required for both steps; declared SHA-256 |
| `file_external.title` | string | Optional on either step; applied on `complete` |
| `file_external.initial_comment` | string | Optional on either step; applied on `complete` |
| `file_external.reply_to` | string | Optional parent message for either step |
| `file_external.file_id` | string | Required for `complete`; returned by `begin` |
| `reaction.message_id` | string | Required, non-empty after trimming |
| `reaction.reaction` | string | Required; lower-case letters, digits, `_`, `+`, or `-`, maximum `100` characters |
| `reaction.action` | literal | Required: `add` or `remove` |
| `message_id` (edit/delete) | string | Required, non-empty after trimming |
| `text` (edit) | string | Required, non-empty after trimming |
| `files` (file or file_external) | array of 1–10 objects | Alternative to single-file fields; described below |
| `coordination_operation_id` (message/file) | UUID | Managed reply only: Cloud reply intent ID; requires `reply_to`, `expected_generation`, `request_id` |
| `expected_generation` (message/file) | non-negative integer | Managed reply only: claim generation the intent was registered under |
| `request_id` (message/file) | UUID | Managed reply only: caller idempotency key for this reply |

The variant prefixes above indicate when a field applies. `file` and reply entries in `replies[]` are nested input objects; `text`, `step`, `filename`, `message_id`, and the other
variant fields are top-level arguments. For example:

```json
{"channel":"cas-scratch","kind":"file","file":{"filename":"notes.txt","content":"hello","content_encoding":"text"}}
```

```json
{"channel":"cas-scratch","kind":"edit","message_id":"1710000000.000002","text":"Updated announcement"}
```

### Managed replies

A thread in a managed binding accepts replies only from its current Cloud
claim owner (spec-022d §4, §6, §7). The owner registers a reply intent with
Cloud (`POST /api/coordination/external-work/{id}/reply-intents`), then calls
`kind:"message"` or `kind:"file"` with `reply_to` and all three fields:

```json
{"channel":"coord","kind":"message","text":"On it.","reply_to":"1760616000.123456",
 "coordination_operation_id":"00007e91-1111-4111-8111-000000007e91","expected_generation":1,
 "request_id":"5b0c1f2e-3d4a-4b5c-8d6e-7f8091a2b3c4"}
```

The three fields go together and only with `reply_to`; otherwise the call is
`invalid_input`. Supplying them on an unmanaged channel also uses the gateway.
The gateway then:

1. Persists the operation in `reply_operations` (migration 008), keyed by
   `request_id`. The row holds an immutable digest of the request, never the
   text or bytes.
2. Consumes the intent at Cloud
   `POST /api/coordination/reply-intents/{id}/dispatch` (`transition:"consume"`,
   `claim_generation` = `expected_generation`), using the hub's relay key.
   Cloud must confirm `dispatching` for this channel and `reply_to` thread.
3. Posts to Slack exactly once, through a client with SDK retries disabled.
4. Records `posted` with the Slack `provider_receipt`, `rejected` when Slack
   provably refused, or `outcome_unknown` when the result is uncertain.

Cloud calls share one 8-second budget per request, each capped at 5 seconds.
Each transition uses a request ID derived from `request_id`, so a resumed
request replays on Cloud instead of mutating twice.

The receipt is the ordinary `message`/`file` receipt plus `coordination`:
`{request_id, coordination_operation_id, generation, state, recorded}`.
`state` is `posted`, `rejected`, `outcome_unknown` or `refused`. `recorded` says
whether Cloud acknowledged the final transition; `recorded:false` after a post
means Cloud still holds the thread pinned until reconciliation. A Slack upload
that returns no message ts is `outcome_unknown`.

Replaying the same `request_id` with the same input returns the stored result
with `coordination.replayed:true`, without calling Slack or Cloud. Cassy
metadata fields are not part of the input identity. Changed input returns
`idempotency_conflict`.

| Code | Meaning | Slack called |
| --- | --- | --- |
| `managed_reply_required` | Reply into a managed thread without the fields (message, file or `file_external` complete). Omitting them is not a bypass. | No |
| `stale_generation` | Cloud says the claim moved on (`detail` carries Cloud's reason) | No |
| `reply_refused` | Cloud refused the consume (`detail`: `invalid_transition`, `claim_not_active`, `unknown_resource`, …) | No |
| `reply_unresolved` | Another reply in this thread is `reserved`, `dispatching` or `outcome_unknown`, here or at Cloud | No |
| `reply_target_mismatch` | The intent belongs to another thread; the hub records `rejected` at Cloud | No |
| `idempotency_conflict` | `request_id` reused with different input | No |
| `operation_pending` | The same `request_id` is still in flight; retry the same request later (retryable) | No |
| `authority_unavailable` | Relay key, reply store or Cloud unavailable. Retryable after an uncertain consume: retry the **same** `request_id` | No |
| `outcome_unknown` | Slack may have posted. Not retryable, never reposted; the thread stays pinned | Once |

`outcome_unknown` is final for the caller. Do not retry with a new
`request_id`. Reconcile from Slack evidence, then settle the row to `posted`
or `rejected`; the operator query is at the end of
`apps/violet/db/migrations/008_reply_operations.sql`. Cloud is reconciled
through its own `posted`/`rejected` transitions. Until then, managed replies
in that thread are refused with `reply_unresolved`.

These are unchanged: non-reply posts, reactions, edits, deletes, ordered
threads, `file_external` begin, unmanaged channels, and people posting in Slack
directly. If the managed-binding lookup fails, the reply fails closed with
`hub_unavailable`.

### Ordered thread post

`kind:"thread"` requires top-level `text`, `idempotency_key` (1–200 characters),
and `replies` (1–20 entries). Each reply contains `text` and/or `files` (1–10).
All inline files across the entire call must total at most `1048576` decoded
bytes. Inline files use the same nested fields as `kind:file`; external files
use `{file_id,filename,size_bytes,sha256,title?}` after begin/direct upload.
Each reply must use all inline files or all external metadata. External IDs
must be unique throughout the thread. File replies share once with their
text as `initial_comment`; every reply targets the new top-level message.
The hub validates all inline bytes/PNG integrity and declared sizes before
posting the root, and paces writes at least one second apart within the call.

```json
{"channel":"cas-internal","kind":"thread","text":"Live on production · User — Images can accompany release details.","idempotency_key":"release-example-user","replies":[{"text":"Was → separate posts. Now → images stay with the explanation.","files":[{"filename":"notes.txt","content":"hello"}]},{"text":"More detail"}]}
```

The response is `{ok,schema_version:1,kind:"thread",channel,idempotency_key,
posted:[{index,message_id,thread_id,permalink,files?}],failed_index,resume_safe,
error?}`. Index `0` is the root; the first reply is index `1`. Success returns
all message IDs/permalinks in order and `failed_index:null`. A failure sets
MCP `isError:true`, stops immediately and retains all available receipts with
the failing index. File receipts retain `sha256_verified` and the integrity
semantics below. An unresolved file-message receipt is retained, and later
replies stop; it is not silently treated as a completed thread.

Keys are scoped to authenticated caller and resolved channel ID. Identical
content with the same key returns stored receipts without replaying any
successful writes, including across instances and channel aliases.
Different content on the same key returns `invalid_input`. A confirmed
failure before dispatch (for example local pacing contention) has
`resume_safe:true`: retry the identical request/key after the indicated delay.
After dispatch, a missing response, permalink failure, hash failure, interrupted
process or lost checkpoint may leave a message in Slack. Such a thread has
`resume_safe:false`, never automatically retries that item, and requires
inspection. An active concurrent call also returns `ambiguous_post`; the
original may still finish, so a later same-key read may return its receipts.
Do not change the key to bypass a stopped or active record.

The durable journal stores only hashed scope/content and receipts; it keeps
no message text, bytes or credentials. Apply migration
`apps/violet/db/migrations/006_thread_posts.sql` manually. With no journal or
an unapplied migration, only thread posts fail with permanent
`capability_unavailable` and a message that the thread journal is not installed.
Records have no automatic expiration: removing a key removes replay protection.
Slack SDK retries are disabled for thread operations; ordinary post operations
retain their existing transport. The route permits up to 60 seconds, but a
hosting deadline can still interrupt a slow thread; inspect active records.

Local paths are client inputs, not hub inputs. For a draft with `files:[{path,
title?}]`, `scripts/slack-thread-input.py --channel CHANNEL DRAFT_JSON` reads
paths relative to the draft, encodes bytes and supplies length/SHA-256.
`scripts/slack-post.sh post --channel CHANNEL --thread DRAFT_JSON` prepares
that draft and makes one `violet_post` thread call. Larger files must already
be uploaded externally before their metadata is put in the draft.

Successful message and file receipts use these shapes:

```json
{
  "ok": true,
  "schema_version": 1,
  "kind": "message",
  "channel": {"id": "C123", "name": "cas-internal"},
  "message": {"message_id": "1710000000.000002", "thread_id": "1710000000.000002", "permalink": "https://slack.test/p/2"}
}
```

```json
{
  "ok": true,
  "schema_version": 1,
  "kind": "file",
  "channel": {"id": "C123", "name": "cas-internal"},
  "message": {"message_id": "1710000000.000003", "thread_id": null, "permalink": "https://slack.test/f/3"},
  "file": {"file_id": "F123", "name": "notes.txt", "size_bytes": 5, "sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824", "sha256_verified": true, "state": "attached", "image_validated": null, "permalink": "https://slack.test/f/3"},
  "warning": null
}
```

File receipts report the bytes sent to Slack: `size_bytes` and lowercase
`sha256` are always present. `state` is `attached` when Slack returns a
resolved channel or thread message and `uploaded` when the file permalink is
confirmed but its message timestamp cannot be resolved. `image_validated` is
`true` for a valid PNG (when `filename` ends in `.png` or `content_type` is
`image/png`) and `null` for non-PNG files. A PNG with an invalid chunk CRC or
invalid IHDR/IEND structure is rejected before Slack I/O.

When supplied, `file.sha256` and `file.size_bytes` are checked against the
decoded bytes before any Slack write. A mismatch returns
`error.code: "file_integrity_mismatch"`, `retryable: false`, and a `detail`
of `sha256` or `size_bytes`; PNG structure or CRC failures use detail
`png_chunk_crc`.

### External file uploads

For a file larger than `1048576` decoded bytes, call `violet_post` with
`kind: "file_external"` and `step: "begin"`, passing `filename`,
`size_bytes`, `sha256`, and any eventual title, comment, or reply parent. The
successful begin receipt contains `upload_url`, `file_id`, and optional
`expires_at`; stream the file bytes directly to `upload_url` with no hub
credentials. Then call `step: "complete"` with the returned `file_id` and the
same filename, size, hash, and optional metadata. The hub completes the Slack
upload, fetches `files.info`, and refuses a size mismatch with
`file_integrity_mismatch` before returning the normal `kind: "file"` receipt.
An expired or unknown upload returns `file_upload_unavailable` with
`retryable: false`; begin a new upload instead of retrying completion.

### Several attachments in one message

For inline attachments, use `kind: "file"` with `files: [{filename,
content, content_encoding?, content_type?, sha256?, size_bytes?, title?},
...]` instead of `file`. All entries are decoded and validated before upload;
their combined decoded content must fit `1048576` bytes. A single shared
`initial_comment` and optional `reply_to` apply to the message. All bytes are
uploaded first, then one Slack `files.completeUploadExternal` call shares the
batch. No separate text message is posted.

For larger files, use `kind: "file_external"` with `step: "begin"` and
`files: [{filename,size_bytes,sha256,title?}, ...]` instead of the singular
top-level metadata. Begin returns a corresponding `files[]` with metadata,
`file_id`, `upload_url`, and optional `expires_at`. Upload each file's bytes
directly to its own URL. Complete with `step: "complete"`, the same
`files[]` metadata plus each `file_id`, and the shared `initial_comment` and
`reply_to`. Do not send URLs or content in the completion input. File IDs
must be unique. Singular fields cannot be mixed with an array, and every
batch contains 1–10 entries.

```json
{"channel":"cas-scratch","kind":"file_external","step":"complete","files":[{"file_id":"F1","filename":"screenshot.png","size_bytes":12345,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},{"file_id":"F2","filename":"walkthrough.webm","size_bytes":3974709,"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}],"initial_comment":"Screenshot and walkthrough","reply_to":"1710000000.000002"}
```

Multi-file completion returns the usual channel and one `message` receipt
plus `files[]`, where each entry has `file_id`, `name`, `size_bytes`,
`sha256`, `sha256_verified`, `state`, `image_validated`, and `permalink`. The message permalink
refers to the shared Slack message. If its timestamp cannot be resolved,
`message_id` and the message permalink are null, each file retains its own
permalink, and `warning.code` is `message_id_unresolved`. Timestamp recovery
requires a message containing every batch file ID. The existing single-file
input and `file` receipt shape remain supported; `sha256_verified` is additive.

Slack-hosted images (`files.info.mode: "hosted"`, `is_external: false`, and an
`image/*` MIME type) return `sha256_verified: false` and
`verification_skipped: "slack_transformed_image"`. Slack asynchronously
re-encodes images, including after a matching download: PNG, JPEG and WebP
changed bytes in both single-file and multi-file probes. The GIF probe kept
its bytes, but all hosted images conservatively use this rule. The reason
means Slack may transform the image, not that a transformation was proven
for this particular file. The hub neither compares their post-share size/hash
nor deletes them for such differences. `sha256` remains the submitted hash;
a returned Slack size may differ from the submitted size. Inline input size,
hash and PNG CRC validation still happen before upload. Initially blank Slack
MIME/filetype classification is retried before deciding how to verify; if it
stays pending, verification is unavailable and the file is preserved.

For other files, completion verifies each Slack-reported size after sharing.
For files at or below `16777216` bytes (16 MiB), the hub streams Slack's private download
URL with bot authentication and compares its SHA-256 to the declaration
(or the locally decoded hash for inline files). A matching digest returns
`sha256_verified: true`. Files above that verification cap or without a hash
return `sha256_verified: false`; their `sha256` is still the declaration.
Pending metadata/classification is polled until the shared batch deadline,
with 250 ms, 500 ms, then capped 1000 ms backoff. These readiness polls do not
consume the separate allowance for other verification failures: transient
metadata errors and download failures get at most three attempts with 250 ms
and 750 ms backoff. Each download gets a base 2.5 seconds plus 1 second per
MiB of its declared size. The batch budget starts at 12 seconds, adds 1 second
per MiB of total declared bytes eligible for hashing, and adds 2.5 seconds for
each additional hashed file. The shared verification budget is capped at
40 seconds, and every attempt is limited to the remaining budget. Files over
the 16 MiB hashing cap or without a hash do not increase this budget. Metadata
calls are capped at 2 seconds. Receipt recovery may take up to 2 further
seconds. Completion is never replayed.

If a shared upload cannot be verified, the response is `ok: false` with
`error.code: "file_verification_unavailable"`, `retryable: false`, and the
message "File posted, verification unavailable. Do not post it again; inspect
the posted receipt." `error.posted` carries `channel_id`, `message_id`,
`thread_id`, `permalink`, and every shared `file_ids` entry. `thread_id` is
null for root posts and the input `reply_to` for replies, matching success
receipts. Message identifiers and permalink may be null if Slack history
cannot resolve them; a file permalink is not substituted for a missing message. This receipt
also survives in a stopped thread's `error.posted`; an identical thread request
returns the journaled outcome without posting again. Structured diagnostics
record the verification stage, attempt, reason and safe upstream status/code;
private download URLs, bodies and tokens are excluded. Read-side failures
continue to use `file_unavailable`.

A digest or downloaded-length mismatch returns `file_integrity_mismatch`
with detail `sha256` or `size_bytes`. After Slack accepts completion, this
error also carries `error.posted` with the same receipt fields and root/reply
semantics as `file_verification_unavailable`. The hub recovers the receipt
before attempting Slack `files.delete` for the mismatched non-image file;
cleanup may remove the message when its only attachment is deleted. Slack can
also accept completion before a message becomes visible. If no message can
be found, `message_id` and `permalink` are null and the error says so. The error
names its `file_id` and `file_cleanup=deleted|unavailable|failed`; other batch
attachments may remain. The receipt survives in stopped thread journals and
replays without another upload. Mismatches rejected before upload have no
`posted` receipt. An error is not proof that nothing was posted. If
begin or an inline byte upload fails mid-batch, allocated uploads may remain
unshared until Slack expires them. Do not automatically replay a failed
completion; inspect receipts and Slack state first.

Reaction receipts add `reaction: {name, action, changed}` and identify the
target message in `message`. Removal calls Slack `reactions.remove`, which
removes only the calling bot's reaction. If Slack reports no caller-owned
reaction while the requested reaction is present, the hub returns permanent
`reaction_not_owned`: only reactions @Violet added can be removed. Have
@Violet add its own `eyes` reaction so it can later swap it for a closeout
reaction. Removing an entirely absent reaction remains successful with
`changed: false`. Missing mutation capability returns permanent
`capability_unavailable`.

Error receipts use
`{ok:false,error:{code,message,retryable}}`, with `retry_after_ms` when a
rate-limit response supplies one. Slack-backed failures may also include the
additive `error.upstream` object with a safe Slack `error` code, HTTP `status`,
and `retry_after` value in seconds when Slack supplied a Retry-After value. It
never contains credentials, request bodies, or channel names. Non-transient
Slack errors such as authentication, scope, and argument errors set
`retryable:false`; transient Slack failures retain the existing retryable code
semantics.

Client and hub failures are distinct from Slack errors. The `/mcp/slack` auth
boundary returns HTTP 401 with `{ok:false,error:{code,message,retryable:false}}`:
`unauthorized` for a missing bearer and `invalid_token` for an unknown or revoked
bearer, preserving the OAuth challenge. For a presented but rejected bearer the
`WWW-Authenticate` challenge says
`error_description="Bearer token not recognized or revoked"`; a missing bearer
keeps `"No authorization provided"`. Unknown internal hub failures and thread
journal outages use `hub_unavailable`; only genuine Slack transport/outage
failures use `upstream_unavailable`. Permanent Slack rejections use `slack_error`.

The shell helper reports these additional boundary codes without printing
bearer/bypass values or arbitrary response text:

| Code | Action |
| --- | --- |
| `missing_credential` | Set the named variable: `VIOLET_SLACK_TOKEN` or `VIOLET_VERCEL_BYPASS`. |
| `invalid_token` / `unauthorized` | HTTP 401/403 carrying the hub's own auth error envelope: repair the hub client bearer or registration. Any other 401/403 is `hub_bad_response` naming the requested host. |
| `bypass_rejected` | Protection HTML on HTTP 401/403: repair `VIOLET_VERCEL_BYPASS`. |
| `hub_unreachable` | Check `VIOLET_HUB_URL`, DNS, connection, TLS and timeout; the curl status is included. |
| `hub_bad_response` | Check the hub URL/version; HTTP status and a fully redacted structural response prefix identify wrong endpoints or invalid JSON/SSE/tool payloads. |
| `local_request_failed` | Repair local files, helper dependencies, temporary storage or request construction. |

An unconfirmed write must be inspected before retrying, even on a network error.

Edit receipts use `kind: "edit"` with the target's `message_id`, `thread_id`,
and `permalink` in `message`. Delete receipts use `kind: "delete"` with the
deleted `message_id` in `message`; both operations remain at schema version 1.

## Slack events endpoint (push-wake, not an MCP tool)

`POST /api/slack/events` is the Slack Events API Request URL. It is not part of
the MCP surface and needs no client bearer; Slack's v0 request signature is the
authentication. Operator setup is in `docs/runbooks/violet-events.md`.

- Signature: `X-Slack-Signature` must equal `v0=` + hex HMAC-SHA256 of
  `SLACK_SIGNING_SECRET` over `v0:{X-Slack-Request-Timestamp}:{raw body}`, and
  the timestamp must be within 300 s. Otherwise `401 {"error":"unauthorized"}`.
  With no signing secret configured: `503 {"error":"events_disabled"}`.
- `url_verification` returns `{"challenge": …}`.
- Kept events: `app_mention` by a person, and a person's reply in a thread
  Violet started (`parent_user_id` is Violet) that does not itself mention
  Violet. Bots, Violet's own messages, edits, deletes, joins and DMs are
  dropped with `200`.
- Unmapped channels are recorded as `dropped_unmapped`; nothing leaves the hub.
- Mapped events are forwarded after the `200` to
  `{VIOLET_CLOUD_ENDPOINT}/api/violet/activity` as this envelope (no message
  text, files, blocks or tokens):

```json
{
  "schema": "violet.slack_activity/v1",
  "dedupe_key": "slack:<team>:<channel>:<message ts>",
  "event_id": "Ev…",
  "kind": "mention | thread_reply",
  "project_id": "github.com/<owner>/<repo>",
  "channel": { "id": "G…", "name": "<project>-internal" },
  "message": {
    "ts": "…", "thread_ts": "<thread root>", "user_id": "U…",
    "mentions": { "user_ids": ["U020L9Q9G03"], "violet": false, "broadcast": null }
  },
  "occurred_at": "RFC 3339",
  "received_at": "RFC 3339"
}
```

`thread_ts` is the thread root (the message's own `ts` for a top-level
mention). Consumers ignore unknown `kind` values; `schema` changes only for a
breaking change. Content is read through `violet_read` with `thread_id`.
`message.mentions` uses the read-message contract above and is present on all
new envelopes. This is additive under `violet.slack_activity/v1`; historical
stored envelopes may omit it. Missing metadata means unknown addressees: read
the thread before deciding to reply.

Cloud [#143](https://github.com/Richards-LLC/petra-stella-cloud/issues/143) must
accept and preserve `message.mentions` through storage, claim and relay. Cassy
[#1145](https://github.com/Richards-LLC/cassy/issues/1145) must carry it into the
wake so responder selection (#1146) can wait for a named human without a read.
Event admission, delivery, dedupe, acknowledgements and wake behavior stay the
same; these IDs inform the consuming session’s answer decision.

## Changelog

- **2026-10-16** — Added the managed-reply gateway (violet_ps #50). `violet_post`
  `kind:"message"|"file"` with `reply_to` accepts optional
  `coordination_operation_id`, `expected_generation` and `request_id`. In
  managed threads they are required (`managed_reply_required` otherwise). The
  hub persists the operation (migration 008) and consumes the Cloud reply
  intent before posting once. It records the Slack receipt, or
  `outcome_unknown` without retrying. Receipts gain `coordination`, and new
  stable error codes are listed under "Managed replies". All changes are
  additive, so schema version stays 1. Unmanaged posting is unchanged.

- **2026-10-16** — Added verified Slack `external_ref` thread resolution to
  `violet_read`, with identity-only or targeted-thread receipts. Root/reply/alias
  permalinks converge through the authenticated installation; foreign workspaces
  and non-member channels are refused. Normal messages, old-root context and
  thread failures now include workspace/channel/root identity metadata, counted
  in byte limits. Additive under schema version 1; post receipts are unchanged.

- **2026-10-15** — Hub configuration now reads only `VIOLET_*` environment
  variables. Server-side `MECHA_*` fallback is removed; client helper scripts
  continue to accept legacy names.

- **2026-10-09** — `latest` counts kept root messages, fetching further
  history pages until it has N roots or history ends, so `latest: 1` never
  returns zero messages when the newest history item is a thread reply.

- **2026-10-09** — Added the optional `latest` read input: newest-first slices
  with `complete:false` and a continuation `cursor`, so a one-message read of a
  busy channel succeeds. `max_messages` keeps its complete-digest meaning.
  Additive under schema version 1. The shell client's `read --limit N` now
  sends `latest: N` (`--all` for a complete window, `--cursor` to page).

- **2026-10-09** — A rejected bearer's `WWW-Authenticate` challenge now names
  the rejection instead of "No authorization provided".

- **2026-10-09** — Write paths (membership proof, posts, uploads, reactions,
  edits/deletes) now report permanent Slack rejections such as `invalid_auth`
  as non-retryable `slack_error` with safe `upstream`, matching reads; only
  5xx/fatal/timeout/transport failures stay `upstream_unavailable`.

- **2026-10-09** — Digest-limit `size_cap_exceeded` now includes a count-only
  `detail` (messages vs `max_messages`, files vs `max_files`, envelope bytes vs
  budget); message, code and retryability unchanged.

- **2026-10-09** — Retired transition tool aliases; renamed the app to apps/violet
  and introduced VIOLET_* configuration with one-release fallback.

- **2026-10-09** — Isolated unreadable attachments in `violet_read` (#40):
  added `access_denied`, `file_not_found`, and `file_unreadable` skip reasons,
  safe per-file `upstream` detail and nullable unavailable metadata. Messages
  and readable files survive; wholly unreadable file-only scopes return a
  permanent specific error. Read schema version remains 1.

- **2026-10-09** — Added `mentions` to every read message and `message.mentions`
  to new text-free activity envelopes (#39). Read schema version 1 and activity
  v1 remain unchanged; Cloud #143 and Cassy #1145 must preserve the additive
  field for human handoffs in Cassy #1146.

- **2026-10-09** — Added default-on recent-reply discovery on older roots with a configurable 7-day lookback and minimal `thread_roots` context; reaction receipts now include reactor `users` IDs. Both fields/options are additive under schema version 1.

- **2026-10-09** — Added the `POST /api/slack/events` push-wake endpoint and
  the `violet.slack_activity/v1` envelope. No MCP tool changed; schema version
  remains 1.

- **2026-10-09** — Added bounded multi-file inline and external uploads with
  one shared message receipt; retained single-file behavior and the safe
  inline limit, documented external ceiling and upload-specific size errors.
  Added streamed SHA-256 checks through 16 MiB, explicit per-file verification
  flags, and best-effort deletion of mismatched shared files.
  Reaction removal now distinguishes another user's reaction from an absent
  reaction and reports a permanent capability error when removal is unavailable.
- **2026-10-09** — Flattened the served post input schema for MCP client
  discovery, with every kind and external-upload step documented. Added
  field-specific validation errors and accepted, ignored Cassy publication
  metadata on every kind. Both post names retain identical behavior under
  schema version 1.
- **2026-10-09** — Added file/message/thread scopes, `include_channels` opt-out,
  a 100000-byte default serialized MCP result budget (explicit larger budgets
  retained), and aggregate file skips with optional SHA-256 receipts capped at
  16 MiB; hash failures preserve the rest of the read.
- **2026-10-09** — Added partial read receipts, per-thread failures and opaque continuation cursors; documented the shared 50-page retrieval bound. Skin-tone reactions and file-only replies parse without retryable outage errors.

- **2026-09-29** — Added canonical `violet_read` and `violet_post` tools. The
  transition aliases initially retained identical behavior;
  receipt schema version remains 1.
- **2026-09-18** — Added `resultType: "complete"` to every `tools/call` result
  for MCP protocol revision `2026-07-28`; tool receipt schema version remains 1.
- **2026-09-18** — Added declared file size and SHA-256 checks, PNG integrity
  validation, and additive upload state/hash/validation fields to file receipts.
- **2026-09-18** — Added skipped-file metadata and `skipped_files` to
  the read receipt (today's `violet_read`); files over `max_file_bytes` no longer fail the whole digest and
  do not count toward `max_files`.
- **2026-09-18** — Added the optional `error.upstream` Slack diagnostic object
  and retained schema version 1 because the error envelope change is additive.
- **2026-09-18** — Added the two-step `file_external` upload for artifacts over
  `1048576` bytes. Clients stream bytes directly to Slack, then complete with
  the same metadata; the hub verifies the completed size and returns the
  existing `kind: "file"` receipt.
- **2026-09-04** — Added `channels` to the read receipt (today's `violet_read`). It lists the
  bot's current member channels and is additive under schema version 1.
- **2026-09-03** — Renamed the public tools from `slack_*` to the first coarse
  read/post pair (today's `violet_read` and `violet_post`). This was a breaking tool rename and should have been announced
  to consumers before deployment.

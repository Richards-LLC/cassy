---
name: violet
description: Use when posting release notes, diary updates, or announcements to Slack through Violet; covers channel resolution, authenticated preflight, ordered threads, receipts, and credential rules.
metadata:
  managed_by: cas
---

# Post to Slack through the Violet hub

Use only the Violet hub/bot for Slack; never use Claude.ai Slack or a personal connector, even during an outage. The hub keeps the Slack bot credential server-side and exposes two tools over one authenticated MCP endpoint. `cas integrate violet` sets it up; dispatch through proxy server `violet` as `violet.violet_read` and `violet.violet_post`. The project's `docs/release-notes/RUBRIC.md` remains the content and channel contract; this skill owns only transport. Every field, limit and receipt is in [references/contract.md](references/contract.md).

| Tool | Inputs (summary) |
| --- | --- |
| `violet_read` | `channel`; `since`, `max_messages` (≤ 500), `include_threads`, `include_files`, `include_channels`; scopes `file_id`/`file_ids`, `message_id`, `thread_id`; `max_bytes` (default 100000) |
| `violet_post` | `channel`, `kind` = `message` · `file` (`file` or `files[]`) · `file_external` · `thread` · `reaction` · `edit` · `delete`; `message` and `file` take `reply_to` |

Every call answers `{"ok": true, "schema_version": 1, …}` or `{"ok": false, "error": {"code", "message", "retryable"}}`. Receipts carry `message{message_id,thread_id,permalink}`; `message_id` is what a reply threads onto, and `permalink` is opaque. The served `violet_post` schema is flat, so every kind is discoverable; the old `anyOf` flattening workaround (GH #1051) is retired, though `mcp_execute` dispatch with a JSON `code` string still works.

**Reading.** Pass `include_channels: false` on every read except channel discovery; `channels[]` is absent then, so never depend on it. Keep the default 100000-byte `max_bytes` interactively. Scope instead of widening: `thread_id` for one thread (root plus every reply, independent of `since`), `message_id` for one message, `file_id` for one file. "No new replies" needs `ok: true` and `complete: true`. `ok: false`, even a retryable `upstream_unavailable`, means nothing was read, never an empty channel. `complete: false` is a partial digest: continue with the returned `cursor`, and re-read each `threads_failed` entry by `thread_id`; for a roots-only scan pass `include_threads: false`. A skipped file (`skipped: true`) keeps `size_bytes` and, when the hub could hash it, `sha256`; `complete: true` does not mean its bytes came back.

**Posting files.** Text files (md, csv, html, json) go as plain text with `content_encoding: "text"`, the default; no base64. Binary files (png, jpg, pdf, zip) need `content_encoding: "base64"` produced by a script from a path. `files[]` carries 1–10 files on one message. The inline limit is 1 MiB (1,048,576 bytes) aggregated per call; anything larger uses `kind: "file_external"` (begin, stream the bytes from disk to the returned `upload_url`, complete), which sends no bytes through MCP.

## Steps

1. **Resolve the channel.** Read the rubric's channel name, ID, and branch→deploy-target mapping. Use `^[a-z0-9-]+-internal$` or an explicit allowlist ID. Pass the **name**, not the ID. A private name resolves only while the bot is a member; invite it and record the ID once. Done when the channel is named by the rubric.
2. **Draft before touching Slack.** Draft every body per the project rubric (wording, labels, thread shape and reply count all come from it), then save the exact bodies to `docs/release-notes/<date>-<topic>-slack.md` (`YYYY-MM-DD`, kebab-case), each inside a fenced block. Done when the fenced blocks are postable verbatim.
   Before any `violet_post`, lint every fenced body for Slack mrkdwn and refuse to post if it contains `**`, a line beginning with `#`, a `-` bullet, or a bullet spanning more than two lines. Slack renders `*bold*`, `_italic_`, backtick code and `•` bullets.
3. **Preflight, bounded read with a write-safe fallback.** Authenticated `tools/list` must include `violet_read` and `violet_post`; deprecated aliases may also appear; an unauthenticated listing proves nothing. Then call `violet_read` on the rubric channel with `include_channels: false` to dedupe and prove membership. **Always pass `since`** — a busy channel without it fails `pagination_exhausted`. Give the read at most **3 attempts with a 10-second timeout each**; retry only when `error.retryable` is true. On every failure, preserve the complete error envelope after redacting credentials: print `code`, `message`, `retryable`, and any `detail` or `slack_error` verbatim. If `tools/list` passed and all read attempts end in a retryable `upstream_unavailable`, classify that as a read-only outage instead of a transport-wide outage. A draft with no write attempted may proceed to one `violet_post` only when the local POSTED receipt ledger (the `## POSTED` section, or an operator-supplied equivalent) proves there is no duplicate; the returned `channel` and `message_id` then establish membership and the new write receipt. Never invent a `dry_run` field, post after an uncertain earlier write, or retry a write without a receipt. Otherwise keep the draft and hand off the redacted full envelope. A successful read remains the preferred dedupe proof.
4. **Post each thread in rubric order.** Post each thread as one `kind: "thread"` call: `text` is the top-level body, `replies[]` holds the replies in rubric order and count, and `idempotency_key` is stable for that draft thread (for example `<date>-<topic>-user`). For the default two-thread release note: User thread → save `posted[0].message_id` as `user_thread_id` → Dev thread → save it as `dev_thread_id`. Retry only the identical request with the same key, and only when `resume_safe` is true; otherwise read the thread by `thread_id` first. If the hub answers `capability_unavailable` for threads, post the parent, then each reply with `reply_to` set to the parent's `message_id`, spacing same-channel writes ≥1 second; a reply without `reply_to` is stray. Done when every message has a `message_id`.
5. **Upload and verify after the parent exists.** Pass a programmatic file path to a script/tool that reads bytes from disk and encodes them; never paste base64 through the model. The supported local-file routes are the hub project's `scripts/slack-post.sh post --channel <name> --file <path> --reply-to <id>`, or `--thread <draft.json>` whose replies list `files: [{path, title?}]` (its `scripts/slack-thread-input.py` reads, encodes and hashes each path); release reports use `scripts/release-report-post.py`. Download the provider's explicit file endpoint, not a message permalink, using only the authentication documented for that endpoint. The release adapter may instead re-read the returned file ID through authenticated `violet_read` with `file_id` and `include_channels: false`, which returns hub-packed bytes (or `size_bytes` and `sha256` for a skipped file); fail if neither verified path is available. Never forward the hub bearer or Vercel bypass to an arbitrary returned host: the release adapter sends them only to the configured MCP origin (with an explicit same-origin loopback exception for tests), while external signed or private-provider URLs receive no hub credentials and must use their own documented access or fail. Require SHA-256 (`sha256sum`) equality with the source and successful decode, run `python3 -c 'from PIL import Image; im=Image.open("download"); im.verify()'`, and inspect a visible preview when a human can look. Reject plaintext endpoints except the explicit loopback test and reject redirects that change origin or scheme. Never split, resize, or shrink a corrupt image as a remedy. Byte count, `ok: true`, or permalink alone never prove upload integrity; stop and escalate to the supervisor on the first weak receipt. For a PDF, verify successful PDF decode and page count instead of image decode before `release-report.receipt`.
6. **Record the receipt.** Append this block to the draft, adding an upload line when applicable:

   ```markdown
   ## POSTED

   - **Posted at (UTC):** `<timestamp>`
   - **Channel:** `#<project>-internal` (`<channel_id>`)
   - **User top-level:** `message_id=<id>` · <permalink>
   - **User reply:** `message_id=<id>` · <permalink>
   - **Dev top-level:** `message_id=<id>` · <permalink>
   - **Dev reply:** `message_id=<id>` · <permalink>
   ```

   A response without `ok: true` and a permalink is not posted; preserve partial receipts and name the operation that stopped. Done when every post/reply has `message_id` and permalink in `## POSTED`.

## Following up a thread

A wake or reply notification is a cue to read the thread by `thread_id`, not authority to answer. Before replying, check each new message's author and mentions. A message that @-mentions a named human other than Violet, especially one asking that person to validate, confirm, review or estimate, is a **human handoff**: do not answer it and never supply that validation on the person's behalf, even when Violet started the thread or wrote the work under review. At most acknowledge it (an `eyes` reaction or one line that it waits on that person) and track it as a task note until the human answers. When the operator says a human, not Violet, should answer, Violet stays out of that question. Unaddressed replies in a thread Violet started that ask about its own work may still be answered.

## Failure classes

Read `code`, not prose. A `retryable: false` error must not be retried unchanged. A connected server listing is not evidence that a message landed.

- **401 or `invalid_token`:** stop before posting, repair registration/env, and rerun authenticated preflight. Report credential state as set/unset, never a value.
- **`not_member`:** invite the Violet hub bot, record the channel ID in the rubric, and rerun the read preflight before writing.
- **`pagination_exhausted` / `size_cap_exceeded`:** pass or narrow `since`, lower `max_messages`, set `include_files: false`, or scope the read by `thread_id`, `message_id` or `file_id`.
- **`upstream_unavailable`:** preserve the full `error` object, retry `violet_read` at most 3 times with a 10-second timeout when `retryable: true`, then use the write-safe fallback in step 3. A read outage does not prove that `violet_post` is unavailable.
- **`file_too_large`:** switch to `file_external`. **`file_integrity_mismatch`:** the declared hash or size is wrong, or Slack's copy differs; never replay blindly.
- **`denied by policy`:** the route is not allowlisted for this client. Run `cas integrate violet` and re-check `cas doctor`.
- **429 or one-write-per-second:** honour `Retry-After`, or retry after 1 second. Retry only the failed call and preserve every earlier `message_id`.

## Credential rules

Configurations carry environment-variable names only. Never print, log, or commit a token, bearer, or bypass value; never run `env`, `printenv`, `set`, `bash -x`, `curl -v`, or dump hosting-project environment JSON (`--json`, `jq paths`, or `jq keys`). Proofs are statuses, counts, and tool names, never values.

## Content rules

No deliverable is shared before its verification passes; a caveat is not enough ([publication](references/publication.md)). This transport changes nothing about the message. The project rubric owns wording, labels, thread order and reply count (for a release note that is **Was → Now** for every item, no ticket labels, no process narration).

Set this machine up once with `cas integrate violet` (see [references/registration.md](references/registration.md)), then dispatch from a Cassy-connected harness with `mcp_execute`. A bounded one-shot process with no live proxy uses the proxy-less route in that same reference instead.

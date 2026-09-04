---
name: mecha-cassy
description: Use when an agent must post a Slack message — release notes, a diary update, or an announcement — through the MechaCassy hub. Covers channel resolution, preflight, thread order, receipts, and credential rules.
managed_by: cas
---

# Post to Slack through the MechaCassy hub

The hub keeps the Slack bot credential server-side and exposes two tools over one authenticated MCP endpoint. The project's `docs/release-notes/RUBRIC.md` remains the content and channel contract; this skill owns only transport.

Endpoint `https://mecha-cassy.vercel.app/mcp/slack` exposes exactly:

| Tool | Inputs | Returns |
|---|---|---|
| `mecha_read` | `channel`; `since` (RFC3339), `max_messages` (≤ 500, default 200), `mentions_only`, `include_threads`, `include_files`, `max_files` (≤ 50), `max_file_bytes` (≤ 4194304), `max_bytes` (≤ 8388608) | `channel`, `messages[]`, `files[]`, `counts`, `complete` |
| `mecha_post` | `channel`, `kind`, and the fields for that kind: `message` → `text`; `file` → `file{filename,content,content_encoding,title?}`, `initial_comment?`; `reaction` → `message_id`, `reaction`, `action`. `message` and `file` also take `reply_to`. | `kind`, `channel`, `message{message_id,thread_id,permalink}` |

Every call answers `{"ok": true, "schema_version": 1, …}` or `{"ok": false, "error": {"code", "message", "retryable"}}`. There is no `ts` field and no separate upload tool.

A `mecha_post` receipt looks like this, with a per-kind block alongside it:

```json
{"ok": true, "schema_version": 1, "kind": "message",
 "channel": {"id": "C0…", "name": "cas-internal"},
 "message": {"message_id": "…", "thread_id": "…", "permalink": "https://…"}}
```

`message_id` is the receipt a reply threads onto. A top-level message has `thread_id` equal to its own `message_id`; a reply carries the **parent's** `message_id` as its `thread_id`. Treat `permalink` as opaque — never assemble one by hand.

How far each shape is proven, so you know what to re-check rather than trust: `kind: "message"` is confirmed against real release posts and re-read from the hub. `kind: "reaction"` is confirmed by an add/remove probe. `kind: "file"` is taken from the published input schema and the shared envelope — its per-kind block has not been observed, so treat the first upload of a release as the check, and report what it actually returns.

## Steps

1. **Resolve the channel.** Read the rubric's channel name, ID, and branch→deploy-target mapping. Use `^[a-z0-9-]+-internal$` or an explicit allowlist ID. Pass the **name**, not the ID. A private name resolves only while the bot is a member; invite it and record the ID once. Done when the channel is named by the rubric.
2. **Draft before touching Slack.** Write the exact four bodies — user top-level, user reply, dev top-level, dev reply — to `docs/release-notes/<date>-<topic>-slack.md` (`YYYY-MM-DD`, kebab-case). Done when the file is postable verbatim.
3. **Preflight, exactly two checks.** Authenticated `tools/list` must show exactly `mecha_read` and `mecha_post`; an unauthenticated listing proves nothing. Then call `mecha_read` on the rubric channel to dedupe and prove membership. **Always pass `since`** — a busy channel without it fails `pagination_exhausted`, because the hub returns one complete bounded digest rather than a page. Done when both pass; otherwise keep the draft, report only the redacted error, and do not post.
4. **Post in order, one write per second.** User top-level → save `message.message_id` as `user_thread_id` → user reply with `reply_to=user_thread_id` → dev top-level → save `message.message_id` as `dev_thread_id` → dev reply with `reply_to=dev_thread_id`. Space same-channel writes ≥1 second; a reply without `reply_to` is stray, and a reply's `message_id` is never a parent. Done when all four calls return `ok: true` with a `message_id`.
5. **Attach after the parent exists.** Post `kind: "file"` with `file.filename` and `file.content`, one-second spacing, and `reply_to` set to the parent. Use `content_encoding: "base64"` for anything not UTF-8 text; link artifacts over about 1 MB rather than inlining them.
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

## Failure classes

Every failure is `{"ok": false, "error": {"code", "message", "retryable"}}`. Read `code`, not prose.

- **401 or `invalid_token`:** stop before posting, repair registration/env, and rerun authenticated preflight. Report credential state as set/unset, never a value.
- **`not_member`:** the bot is not in that channel. Invite @MechaCassy, record the channel ID in the rubric, and rerun the read preflight before writing.
- **`pagination_exhausted`:** the digest could not be collected within the bound. Pass `since`, or narrow it.
- **`size_cap_exceeded`:** the digest exceeds the byte limits. Narrow `since`, lower `max_messages`, or set `include_files: false`.
- **`denied by policy`:** the route is not allowlisted for this client. Run `cas integrate mecha-cassy` and re-check `cas doctor`.
- **429 or one-write-per-second:** honour `Retry-After`, or retry after 1 second. Retry only the failed call and preserve every earlier `message_id`.

A `retryable: false` error must not be retried unchanged. A connected server listing is not evidence that a message landed.

## Credential rules

Configurations carry environment-variable names only. Never print, log, or commit a token, bearer, or bypass value; never run `env`, `printenv`, `set`, `bash -x`, `curl -v`, or dump hosting-project environment JSON (`--json`, `jq paths`, or `jq keys`). Proofs are statuses, counts, and tool names, never values.

## Content rules

This transport changes nothing about the message: **Was → Now** for every item, no ticket labels or agent/factory/process narration, and one punch per top-level message with its detail in one reply.

Set this machine up once with `cas integrate mecha-cassy` (see [references/registration.md](references/registration.md)), then dispatch from a Cassy-connected harness with `cas__mcp_execute`. A bounded one-shot process with no live proxy uses the proxy-less route in that same reference instead.

This skill is the whole posting contract. If a machine still carries a separate user-level `mecha-cassy-post` skill, it is a stale copy from before this was a builtin: delete it and use this one, because nothing tests a skill that lives outside the repo.

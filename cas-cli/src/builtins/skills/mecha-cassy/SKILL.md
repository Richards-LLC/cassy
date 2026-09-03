---
name: mecha-cassy
description: Use when an agent must post a Slack message — release notes, a diary update, or an announcement — through the MechaCassy hub. Covers channel resolution, preflight, thread order, receipts, and credential rules.
managed_by: cas
---

# Post to Slack through the MechaCassy hub

The hub keeps the Slack bot credential server-side and exposes four tools over one authenticated MCP endpoint. The project's `docs/release-notes/RUBRIC.md` remains the content and channel contract; this skill owns only transport.

Endpoint `https://mecha-cassy.vercel.app/mcp/slack` exposes exactly:

| Tool | Inputs | Returns |
|---|---|---|
| `slack_post_message` | `channel`, `text`, `thread_ts?` | `ts`, `channel_id`, `permalink` |
| `slack_upload_file` | `channel`, `content`, `filename`, optional title/comment/thread | `file_id`, `ts`, `permalink` |
| `slack_read_channel` | `channel`, `limit` (≤ 50), `oldest?` | messages |
| `slack_list_channels` | `types?` | channel IDs, names, privacy |

## Steps

1. **Resolve the channel.** Read the rubric’s channel name, ID, and branch→deploy-target mapping. Use `^[a-z0-9-]+-internal$`, the configured scratch channel, or an explicit allowlist ID. A private name resolves only while the bot is a member; invite it and record the ID once. Done when the channel is named by the rubric.
2. **Draft before touching Slack.** Write the exact four bodies — user top-level, user reply, dev top-level, dev reply — to `docs/release-notes/<date>-<topic>-slack.md` (`YYYY-MM-DD`, kebab-case). Done when the file is postable verbatim.
3. **Preflight, exactly two checks.** Authenticated `tools/list` must show exactly the four tools above; an unauthenticated listing proves nothing. Then call `slack_read_channel` on the rubric channel with `limit` ≤ 50 to dedupe and prove membership. Done when both pass; otherwise keep the draft, report only the redacted error, and do not post.
4. **Post in order, one write per second.** User top-level → save `ts` as `user_thread_ts` → user reply with `thread_ts=user_thread_ts` → dev top-level → save `ts` as `dev_thread_ts` → dev reply with `thread_ts=dev_thread_ts`. Space same-channel writes ≥1 second; a reply without `thread_ts` is stray, and a reply’s `ts` is never a parent. Done when all four calls return `ts` values.
5. **Attach after the parent exists.** Call `slack_upload_file` with content, filename, one-second spacing, and a parent `thread_ts`; link artifacts over about 1 MB. An `ok` upload with `ts: null` is success because sharing is asynchronous: never retry it.
6. **Record the receipt.** Append this block to the draft, adding an upload line with its `file_id` when applicable:

   ```markdown
   ## POSTED

   - **Posted at (UTC):** `<timestamp>`
   - **Channel:** `#<project>-internal` (`<channel_id>`)
   - **User top-level:** `ts=<ts>` · <permalink>
   - **User reply:** `ts=<ts>` · <permalink>
   - **Dev top-level:** `ts=<ts>` · <permalink>
   - **Dev reply:** `ts=<ts>` · <permalink>
   ```

   A response without `ts` and permalink is not posted; preserve partial receipts and name the operation that stopped. Done when every post/reply has `ts` and permalink in `## POSTED`.

## Failure classes

- **401 or `invalid_token`:** stop before posting, repair registration/env, and rerun authenticated preflight. Report credential state as set/unset, never a value.
- **`not_in_channel` or “invite @MechaCassy”:** invite the bot, record the channel ID in the rubric, and rerun the read preflight before writing.
- **429 or `Slack writes are limited to one per second per channel`:** honour `Retry-After`, or retry after 1 second for the hub message. Retry only the failed call and preserve every earlier `ts`.

Other responses missing `ts` and permalink are unsuccessful. A connected server listing is not evidence that a message landed.

## Credential rules

Configurations carry environment-variable names only. Never print, log, or commit a token, bearer, or bypass value; never run `env`, `printenv`, `set`, `bash -x`, `curl -v`, or dump hosting-project environment JSON (`--json`, `jq paths`, or `jq keys`). Proofs are statuses, counts, and tool names, never values.

## Content rules

This transport changes nothing about the message: **Was → Now** for every item, no ticket labels or agent/factory/process narration, and one punch per top-level message with its detail in one reply.

Register the hub with [references/registration.md](references/registration.md) and dispatch from a Cassy-connected harness with `mcp__cas__mcp_execute`.

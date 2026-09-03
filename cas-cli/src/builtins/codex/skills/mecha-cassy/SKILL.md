---
name: mecha-cassy
description: Use when an agent must post a Slack message — release notes, a diary update, an announcement — through the MechaCassy hub, the default Slack transport for every harness. Covers channel resolution, preflight, thread order, receipts, and credential rules.
managed_by: cas
---

# Post to Slack through the MechaCassy hub

The hub keeps the Slack bot credential server-side and exposes four tools over
one authenticated MCP endpoint, so any harness on any account posts as the same
bot with threads, file uploads, and permalink receipts. The project's
`docs/release-notes/RUBRIC.md` remains the content and channel contract; this
skill is only the transport.

Endpoint `https://mecha-cassy.vercel.app/mcp/slack` exposes exactly four tools:

| Tool | Inputs | Returns |
|---|---|---|
| `slack_post_message` | `channel`, `text` (mrkdwn), `thread_ts?` | `ts`, `channel_id`, `permalink` |
| `slack_upload_file` | `channel`, `content`, `filename`, `title?`, `initial_comment?`, `thread_ts?`, `content_encoding?` | `file_id`, `ts`, `permalink` |
| `slack_read_channel` | `channel`, `limit` (≤ 50), `oldest?` | `[{ts,user,text}]` |
| `slack_list_channels` | `types?` | `[{id,name,is_private}]` |

## Steps

1. **Resolve the channel.** Read the rubric for the channel name, its ID, and
   the branch → deploy-target mapping. The hub permits a channel whose name
   matches `^[a-z0-9-]+-internal$`, or the configured scratch channel, or an ID
   on its explicit allowlist; it resolves a private name only while the bot is
   a member. Onboard a project once: invite the bot to `#<project>-internal`
   and record that name and ID in the rubric. Done when you hold a channel the
   rubric names.
2. **Draft before touching Slack.** Write the exact four bodies — user
   top-level, user reply, dev top-level, dev reply — to
   `docs/release-notes/<date>-<topic>-slack.md`, date `YYYY-MM-DD` and topic
   kebab-case. Done when the file on disk is postable verbatim.
3. **Preflight, exactly two checks.** An authenticated `tools/list` must show
   the four tools above and nothing else; an unauthenticated listing proves
   nothing. Then read the rubric channel with `slack_read_channel` and
   `limit` ≤ 50 — that dedupes against an existing announcement and proves
   membership before any write. Done when both pass. If either fails, keep the
   draft, report the redacted error, and do not post.
4. **Post in order, one write per second.** User top-level → save its `ts` as
   `user_thread_ts` → user reply with `thread_ts=user_thread_ts` → dev
   top-level → save its `ts` as `dev_thread_ts` → dev reply with
   `thread_ts=dev_thread_ts`. Space writes to the same channel by at least one
   second. A reply sent without `thread_ts` becomes a stray top-level message,
   and a reply's `ts` is never a parent. Done when four calls returned four
   `ts` values.
5. **Attach only after the parent exists.** `slack_upload_file` with `content`
   and `filename`, the same one-second spacing, and `thread_ts` to hang it
   under a parent. The content limit is about 1 MB; link anything larger from
   the artifacts root instead. An upload that returns `ok` with `ts: null` is a
   success — the share attaches asynchronously — so never retry it, because a
   retry re-uploads the file.
6. **Record the receipt.** Append to the saved draft:

   ```markdown
   ## POSTED

   - **Posted at (UTC):** `<timestamp>`
   - **Channel:** `#<project>-internal` (`<channel_id>`)
   - **User top-level:** `ts=<ts>` · <permalink>
   - **User reply:** `ts=<ts>` · <permalink>
   - **Dev top-level:** `ts=<ts>` · <permalink>
   - **Dev reply:** `ts=<ts>` · <permalink>
   ```

   An upload line records its `file_id`, `ts`, and permalink. A response with
   no `ts` and permalink is not posted: keep the partial receipts and name the
   operation that stopped.

**Done when** every top-level message and reply carries a `ts` and a permalink
in the `## POSTED` block.

## Failure classes

- **401 or `invalid_token`** — the hub rejected the bearer. Stop before
  posting, repair the registration or its environment, and rerun the
  authenticated preflight. Report credential state as `set` or `unset`, never a
  value.
- **`not_in_channel`, or "invite @MechaCassy"** — the bot is not a member, or a
  private name could not resolve. Invite the bot, record the resolved channel
  ID in the rubric, and rerun the read preflight before retrying the write.
- **429, or `Slack writes are limited to one per second per channel`** — honour
  `Retry-After` on a 429 and treat the hub's message as retry-after-1s. Retry
  only the failed call, preserve every earlier `ts`, and never repost a
  successful parent.

Any other response missing `ts` and permalink is an unsuccessful post. A
connected server listing is not evidence that a message landed.

## Credential rules

Registrations carry environment variable *names*. No config file, prompt, task
note, commit, log, or receipt holds a token, bearer, or bypass value. Never run
`env`, `printenv`, `set`, `bash -x`, or `curl -v` around a posting command, and
never dump a hosting project's environment JSON — platforms embed bypass
secrets as object keys, so `--json`, `jq paths`, and `jq keys` leak them.
Proofs are statuses, counts, and tool names, never values.

## Content rules

This transport changes nothing about the message: **Was → Now** for every item,
no internal ticket labels, no agent/factory/process narration, and one punch per
top-level message with its detail in the single reply.

Register the hub for your harness with
[references/registration.md](references/registration.md); dispatch from a
Cassy-connected harness with `mcp__cs__mcp_execute`.

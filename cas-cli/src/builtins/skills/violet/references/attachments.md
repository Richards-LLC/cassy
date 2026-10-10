# Reading a thread with attachments

Violet is the only Slack read route, including for files. The Claude.ai Slack
connector, the Codex Slack app and a personal browser session are not approved
transports, even read-only. When Violet cannot read a file, use the fallback
below rather than a personal route.

## Route

1. **Read the text first.** Call `violet_read` with `thread_id` (or `since`),
   `include_files: false` and `include_channels: false`. Each message's
   `file_ids` names its attachments.
2. **Read the files by ID.** Call `violet_read` with `file_ids` (up to 50). A
   file over the budget comes back `skipped: true` with `size_bytes` and,
   when the hub could hash it, `sha256`. Raise `max_bytes` only from a script.
3. **Isolate a failure.** A single unreadable attachment currently fails the
   whole call (table below). Re-read the IDs one at a time with `file_id` to
   find which file is unreadable; the rest still read.
4. **Report what was not read.** List every unread file ID with its error
   code. Never conclude from the text alone when an attachment could not be
   read.

## What each failing route returns

| Route | Result |
| --- | --- |
| `violet_read`, bot not in the channel | `not_member`, `retryable: false`, message `Invite @Violet to #<name> and retry.` |
| `violet_read`, Slack refuses the file's metadata (Slack Connect `access_denied`, or `file_not_found`) | The whole call fails with `upstream_unavailable`, `retryable: false`, and Slack's code in `error.upstream.error`. This is a permanent denial, not an outage: do not retry it unchanged. |
| `violet_read`, Slack refuses the file download | The whole call fails with `file_unavailable` ("An attached file could not be downloaded.") |
| Claude.ai Slack `slack_read_file` on a file posted by an external org | `execution_failed: file_not_found` (observed 2026-10-05, GH #1129). This route is not approved anyway. |

## Slack Connect files

In a Slack Connect channel, files uploaded by the external org are owned by
that org. Slack documents that a bot token reads `files.info` for files in
channels the bot belongs to. It also documents a Slack Connect-specific
`access_denied` ("Unable to access the file (slack connect)"). It has not been
verified whether a given external org's files open for Violet once it is a
member. The 2026-10-05 channel returned `not_member`. A hub change to skip an
unreadable attachment instead of failing the read is tracked as violet_ps#40.

## Fallback

- On `not_member`, ask the operator in the session, not in Slack, to invite
  @Violet to the channel. Never post in the channel to ask. Then re-run the
  read.
- If a file stays unreadable after the bot is a member, ask the operator to
  re-share it into a channel Violet reads, or to put the file on disk for you.
  Record the unread file IDs in the task note or report until then.

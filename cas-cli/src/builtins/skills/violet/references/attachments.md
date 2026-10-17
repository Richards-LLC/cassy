# Reading a thread with attachments

Violet is the only Slack read route, including for files. The Claude.ai Slack
connector, the Codex Slack app and a personal browser session are not approved
transports, even read-only. When Violet cannot read a file, use the fallback
below rather than a personal route.

## Route

1. **Read the text and the file IDs.** Call `violet_read` with `thread_id`
   (or `since`), `include_files: true`, `max_file_bytes: 1` and
   `include_channels: false`. Each message's `file_ids` names its
   attachments, and each file comes back without its bytes (`skipped: true`,
   `skip_reason: exceeds_max_file_bytes`); the hub may still stream a skipped
   file up to 16 MiB to fill `sha256`, so this read is not metadata-only on
   the hub's side. With `include_files: false` every
   message's `file_ids` is `[]` whether or not it has attachments: presence is
   unknown, never absent.
2. **Read the files by ID.** Call `violet_read` with `file_ids` (up to 50), or
   `file_id` for one. A
   file over the budget comes back `skipped: true` with `size_bytes` and,
   when the hub could hash it, `sha256`. Raise `max_bytes` only from a script.
3. **Find what was skipped.** An attachment Slack refuses (Slack Connect
   `access_denied`, `file_not_found`, or a refused download) does not fail
   the read: it comes back `skipped: true` with `skip_reason` `access_denied`,
   `file_not_found` or `file_unreadable`, Slack's code in `upstream`, and the
   rest of the read intact; the hub's `file_unavailable` failure now lands on
   the file instead of the call. A rate limit, an outage, a transport error or
   any other Slack refusal still fails the whole call (table below).
4. **Report what was not read.** List every skipped or unread file ID with its
   `skip_reason` or error code. Never conclude from the text alone when an
   attachment could not be read.

## What each failing route returns

| Route | Result |
| --- | --- |
| `violet_read`, bot not in the channel | `not_member`, `retryable: false`, message `Invite @Violet to #<name> and retry.` |
| `violet_read`, Slack refuses the file's metadata (Slack Connect `access_denied`, or `file_not_found`) | The read succeeds; that file is `skipped: true` with `skip_reason` `access_denied` or `file_not_found` and Slack's code in `upstream.error`. This is a permanent denial, not an outage: do not retry it unchanged. |
| `violet_read`, Slack refuses the file download | The read succeeds; that file is `skipped: true` with `skip_reason: file_unreadable` and the HTTP status in `upstream.status`. |
| `violet_read`, Slack rate-limits, is down, the connection fails, or Slack refuses with another code | The whole call fails, usually `rate_limited` or `upstream_unavailable`; follow its `retryable` flag. |
| Claude.ai Slack `slack_read_file` on a file posted by an external org | `execution_failed: file_not_found` (observed 2026-10-05, GH #1129). This route is not approved anyway. |

## Slack Connect files

In a Slack Connect channel, files uploaded by the external org are owned by
that org. Slack documents that a bot token reads `files.info` for files in
channels the bot belongs to. It also documents a Slack Connect-specific
`access_denied` ("Unable to access the file (slack connect)"). It has not been
verified whether a given external org's files open for Violet once it is a
member. The 2026-10-05 channel returned `not_member`. An external file Violet
cannot open now comes back skipped with `skip_reason: access_denied` instead of
failing the read.

## Fallback

- On `not_member`, ask the operator in the session, not in Slack, to invite
  @Violet to the channel. Never post in the channel to ask. Then re-run the
  read.
- If a file stays unreadable after the bot is a member, ask the operator to
  re-share it into a channel Violet reads, or to put the file on disk for you.
  Record the unread file IDs in the task note or report until then.

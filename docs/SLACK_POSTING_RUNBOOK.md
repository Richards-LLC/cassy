# Slack posting runbook — publish through MechaCassy

`docs/RELEASE_SLACK_RUBRIC.md` owns content and thread shape. Use only the
MechaCassy hub/bot for Slack. Never use Claude.ai Slack or a personal connector,
including during an outage. A supervisor handoff uses the same hub.

**Target:** `#cas-internal` (`C0B44GUKDK2`); pass the name `cas-internal`.
Never use a production post as a transport probe. During an operational embargo,
use read-only preflight on the designated test channel or DM; any test write
needs explicit authorization.
A previous `Connected` result proves neither current access nor delivery.

## Preflight and transport

Use the builtin [mecha-cassy](../cas-cli/src/builtins/skills/mecha-cassy/SKILL.md)
for the live transport contract and its registration reference for setup.

1. Require authenticated `tools/list` to expose exactly `mecha_read` and
   `mecha_post`. An unauthenticated list or server status is insufficient.
2. Call `mecha_read` on the rubric channel with explicit RFC3339 `since` and
   bounded `max_messages` to prove membership and deduplicate. Allow at most
   three attempts with a 10-second timeout each; retry only `error.retryable`.
3. Preserve full redacted error envelopes (`code`, `message`, `retryable`, and
   any `detail` or `slack_error`). The hub skill's read-only outage procedure
   permits one hub write only after authenticated listing succeeds, all bounded
   reads end in retryable `upstream_unavailable`, no write was attempted, and
   the local POSTED ledger proves no duplicate. Never retry an uncertain write.
4. Use the Cassy proxy or configured direct MechaCassy MCP. A proxy-less
   one-shot follows the same hub registration reference; it does not select a
   personal connector. If the hub cannot complete publication, save the draft
   and partial receipts and report the measured failure to the supervisor.

Configurations name environment variables only. Never print, log or commit
credentials, dump environment/config secrets, enable shell tracing or verbose
HTTP output. Record statuses, tool names and redacted failures only.

## Publication procedure

1. Save the exact reviewed bodies in
   `docs/release-notes/<date>-<topic>-slack.md`, with each body in a fenced block.
   Follow the content rubric and lint Slack mrkdwn before any write. Send the
   fenced body verbatim, excluding fences or handoff separators.
2. If an earlier run died, deduplicate through the bounded read before retrying.
   Preserve every partial receipt; never retry an uncertain earlier write or
   invent a `dry_run` field.
3. Post User top-level → capture `message.message_id` as `user_thread_id` →
   User reply with `reply_to=user_thread_id` → Dev top-level → capture
   `message.message_id` as `dev_thread_id` → Dev reply with
   `reply_to=dev_thread_id`. Space same-channel writes at least one second.
   A reply's own ID is never its parent. Diary-only updates retain their
   separate parent + Grok, Claude, Codex replies in that order.
4. Upload and verify the report as below after its parent exists.
5. Append `## POSTED` with UTC timestamp, channel, every returned message ID
   and permalink. Require `ok: true`; preserve `message.thread_id` to prove
   replies belong to their parents. Treat permalinks as opaque. A server
   listing, successful exit or missing receipt cannot establish publication.

## Release-report PDF upload

The report is a required publication artifact. After
`cas release report <version> --pdf` produces the committed Markdown, brief,
HTML, PDF and QA receipt, attach the exact PDF bytes to the User top-level
thread through `mecha_post` with `kind: file`, `reply_to=user_thread_id` and
`content_encoding: base64`. Read/encode the bytes programmatically from disk;
never transcribe base64 through agent context. Link HTML from the Dev thread.

Download the uploaded PDF and require source SHA-256 equality, successful PDF
decode and matching page count before accepting it. Apply the hub skill's
image decode/preview checks to image uploads. Byte count, `ok: true` or a
permalink alone never proves integrity; preserve partial receipts and stop on
the first weak receipt rather than changing or shrinking the artifact.

Save `release-report.receipt` under the release-train run directory with
`TAG`, `PDF_PATH`, `HTML_PATH`, `PDF_SHA256`, `HTML_SHA256`, `PAGE_COUNT`,
`PDF_FILE_PERMALINK`, `PDF_FILE_ID`, `HTML_FILE_ID`, `USER_THREAD_TS` and
`DEV_THREAD_TS`, alongside the returned thread permalinks. The existing
`*_THREAD_TS` adapter fields carry the returned hub parent `message_id` values;
MechaCassy has no `ts` response field. Never mark the release announced without
all report and publication receipts.

## Worker handoff and one-shots

A worker without hub access saves the exact draft and sends its path, channel,
deploy target and receipt request to the supervisor. The posting owner repeats
authenticated preflight, uses MechaCassy, and returns message IDs/permalinks for
the worker to record. If no hub route succeeds, report blocked; do not switch
accounts or claim `POSTED`.

For bounded one-shots, close stdin (`< /dev/null`), keep the prompt scoped to the
saved draft and rubric channel, and redirect complete output to a file rather
than piping through `tail`. Use low effort for transcription and retain the
exit status and JSON receipts. Follow the configured hub route's authorization;
CLI account eligibility does not authorize another Slack transport.

## Historical transport evidence

The following 2026-08-27 observations are retained as historical evidence only.
Their route decisions are superseded by the MechaCassy-only policy above and
must never be used as current posting authorization.

| Transport | Measured state on 2026-08-27 | Decision |
|---|---|---|
| Claude `claude.ai Slack` MCP on the approved `pippenz@gmail.com` profile (`~/.claude-alt`) | `claude auth status --json` passed the exact account gate and `claude mcp list` reported the Slack server connected. A normal noninteractive read was permission-blocked, while the explicit one-shot mode completed a read and a smoke DM write; receipt: `D076VR4ATTK`, ts `1787836424.011069`, https://petra-stella.slack.com/archives/D076VR4ATTK/p1787836424011069. | **Canonical route.** |
| Codex `codex_apps` Slack plugin | A bounded `codex exec` probe called `list_mcp_resources(server="codex_apps")` and returned no Slack resource, plugin name, or callable Slack tools. | Not available to default Codex workers; do not spend a turn searching for it. |
| CAS Slack bridge (`cas-bridge-router`) | The router was inactive/not installed; `/etc/cas-bridge/config.json`, `/etc/cas-bridge/router.env`, its systemd unit, and `/opt/cas-bridge` were absent. | Not a configured route. |

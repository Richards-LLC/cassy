# Slack posting runbook — how to actually publish release notes

`docs/RELEASE_SLACK_RUBRIC.md` says *what* to post. This says *how* to verify
and use a Slack transport when the current posting session does not expose one.

**Target channel:** `#cas-internal` — channel ID `C0B44GUKDK2`.

Never use a `#cas-internal` post as a transport probe. During an operational
embargo, use a designated test channel, a DM, or a read-only/dry-run seam. A
transport preflight is runtime state and must be repeated for each posting
session; this document does not turn an old `Connected` result into a receipt.

## Transport decision

Use the first route whose preflight succeeds. The canonical cas-src route is
the supervisor-owned Claude route because default Codex workers do not have a
Slack tool surface.

| Transport | Measured state on 2026-08-27 | Decision |
|---|---|---|
| Claude `claude.ai Slack` MCP on the approved `pippenz@gmail.com` profile (`~/.claude-alt`) | `claude auth status --json` passed the exact account gate and `claude mcp list` reported the Slack server connected. A normal noninteractive read was permission-blocked, while the explicit one-shot mode completed a read and a smoke DM write; receipt: `D076VR4ATTK`, ts `1787836424.011069`, https://petra-stella.slack.com/archives/D076VR4ATTK/p1787836424011069. | **Canonical route.** |
| Codex `codex_apps` Slack plugin | A bounded `codex exec` probe called `list_mcp_resources(server="codex_apps")` and returned no Slack resource, plugin name, or callable Slack tools. | Not available to default Codex workers; do not spend a turn searching for it. |
| CAS Slack bridge (`cas-bridge-router`) | The router was inactive/not installed; `/etc/cas-bridge/config.json`, `/etc/cas-bridge/router.env`, its systemd unit, and `/opt/cas-bridge` were absent. | Not a configured route. |

### Cheap preflight

Run the approved-account gate and server check without inspecting credentials:

```bash
CLAUDE_CONFIG_DIR=~/.claude-alt \
  claude auth status --json | jq '{loggedIn,authMethod,apiProvider,email,subscriptionType}'
CLAUDE_CONFIG_DIR=~/.claude-alt claude mcp list
```

The account gate is positive only for `loggedIn: true`, `authMethod:
"claude.ai"`, `apiProvider: "firstParty"`, and
`email: "pippenz@gmail.com"`. The server check must show
`claude.ai Slack ... Connected`.

For a Codex session, call `list_mcp_resources(server="codex_apps")`. Treat the
plugin as available only when the result contains a Slack resource with its
title/plugin name and the session can invoke its read/write tools. A missing
resource is a definitive negative for that worker; do not infer availability
from `~/.codex/config.toml`.

## Canonical route: supervisor-owned approved Claude profile

The posting owner uses a bounded Claude one-shot with the approved profile
after the preflight above. `--permission-mode bypassPermissions` is required
for a noninteractive one-shot whose Slack MCP calls would otherwise wait for a
human approval; keep the prompt narrowly scoped to the saved draft and target
channel. It does not make a failed account or server preflight positive.

```bash
CLAUDE_CONFIG_DIR=~/.claude-alt \
  claude -p --permission-mode bypassPermissions --output-format text \
  "Use the connected claude.ai Slack MCP. Read the 10 most recent messages in channel C0B44GUKDK2 to deduplicate, then post the exact bodies in /path/to/slack-msgs.md in rubric order. Return each Slack timestamp and permalink. Do not post anywhere else." \
  < /dev/null > /path/to/slack-post.out 2>&1
```

The output file is the receipt source. Confirm that every post returned a
timestamp and permalink, and that threaded replies carry their parent
timestamp. During the embargo, replace the target with a pre-approved test
channel or DM and use a smoke-test body; never substitute `#cas-internal`.

If the approved Claude profile cannot complete a read-only preflight, stop
after saving the draft. Report the exact command, exit status, and error in the
task note, then ask the supervisor to restore authorization or provide a
different approved route. Do not claim `POSTED` or invent a receipt.

## Codex-worker handoff

Default Codex workers have no Slack transport. This is a designed handoff, not
a failed fallback:

1. Save the exact user/dev bodies to `docs/release-notes/<date>-<topic>-slack.md`
   or a separator-delimited handoff file.
2. Tell the supervisor the draft path, target channel, deploy target, and that
   the canonical approved Claude route must post on the worker's behalf.
3. The supervisor (or an approved Claude posting owner) runs the preflight,
   deduplicates, posts, and returns timestamps/permalinks.
4. Add the returned receipt to the saved draft before closing the release task.

If no approved transport exists, leave the draft saved and report the posting
duty blocked with the measured preflight failure. Do not search for another
profile, post from an unapproved account, or mark the draft `POSTED`.

## Procedure

**1. Write the exact message bodies to a file first.** Transcription keeps the
rubric wording intact. Use `=== MESSAGE N ... ===` header lines as separators
and tell the posting owner to exclude the headers from sent text.

**2. If a previous attempt died mid-run, verify before retrying.** A killed run
may have posted some messages. Read the target channel first and only retry if
nothing landed. This read is permitted only after the transport preflight and
must use the designated test channel/DM during an embargo.

**3. Post in rubric order.** User top-level → capture `ts` → user reply → Dev
top-level → capture `ts` → Dev reply. A reply without its parent's `ts` is a
stray top-level message.

**4. Record the receipt.** Annotate the saved draft with a `## POSTED` block
containing the UTC timestamp, channel, and permalink for every post/reply.

## One-shot gotchas

1. **Close stdin.** Always append `< /dev/null`; otherwise `claude -p` or
   `codex exec` can wait for input indefinitely.
2. **Keep writes narrow.** Noninteractive Slack writes need the explicit
   permission mode above. Never grant a broad prompt permission to explore or
   modify the repository.
3. **Do not pipe through `tail`.** Redirect to a file and inspect it after the
   process exits so the receipt is preserved.
4. **Use low effort for transcription.** Prewritten release bodies do not need
   a high-reasoning one-shot.

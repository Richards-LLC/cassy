# Register the MechaCassy hub

Every request carries two headers: `Authorization: Bearer <per-client token>`,
minted per client label and stored hub-side as a hash so labels revoke
individually, and `x-vercel-protection-bypass: <secret>`, checked at the edge.
Both values live in the machine's credentials file as
`MECHA_SLACK_TOKEN_<LABEL>` and `MECHA_VERCEL_BYPASS` and are exported into the
environment by the login shell. Configurations below name those variables and
never hold their values.

## Cassy proxy — reaches every harness

`.cas/proxy.toml`:

```toml
allowlist = [
  "mecha-cassy.slack_list_channels",
  "mecha-cassy.slack_post_message",
  "mecha-cassy.slack_read_channel",
  "mecha-cassy.slack_upload_file",
]

[servers.mecha-cassy]
transport = "http"
url = "https://mecha-cassy.vercel.app/mcp/slack"
auth = "env:MECHA_SLACK_TOKEN_CASSY_PROXY"

[servers.mecha-cassy.headers]
x-vercel-protection-bypass = "env:MECHA_VERCEL_BYPASS"
```

Dispatch through the proxy:

```text
cas__mcp_execute server=mecha-cassy tool=slack_read_channel args={"channel":"<name or ID>","limit":50}
```

The proxy resolves its bearer when `cas serve` starts, so a variable exported
after startup stays invisible until the next restart. `cas__system
action=proxy_health` is credential-free: the healthy record for `mecha-cassy`
reports `tool_count=4` and no error code. `.cas/proxy_catalog.json` is a
generated cache, not source configuration.

## Codex

`config.toml` under the Codex home:

```toml
[mcp_servers.mecha-cassy]
url = "https://mecha-cassy.vercel.app/mcp/slack"
bearer_token_env_var = "MECHA_SLACK_TOKEN_CASSY_PROXY"
env_http_headers = { "x-vercel-protection-bypass" = "MECHA_VERCEL_BYPASS" }
```

`codex mcp list` must show `mecha-cassy` enabled, naming the bearer variable
rather than a value.

## Claude Code

A user-scope HTTP server in the selected profile's `.claude.json`:

```json
{
  "mcpServers": {
    "mecha-cassy": {
      "type": "http",
      "url": "https://mecha-cassy.vercel.app/mcp/slack",
      "headers": {
        "Authorization": "Bearer ${MECHA_SLACK_TOKEN_<LABEL>}",
        "x-vercel-protection-bypass": "${MECHA_VERCEL_BYPASS}"
      }
    }
  }
}
```

`${VAR}` expands from the process environment **at launch**. A variable
exported inside a running session is never seen by that session: export both
before starting the client, then confirm with `claude mcp list`. Where a
profile has its own configuration directory, run the check with that directory
selected, because a registration written for one profile is invisible to
another.

## Proxy-less one-shot

A bounded `codex exec` or `claude -p` process with no live proxy uses the hub
project's `scripts/slack-post.sh` with `post`, `upload`, `read`, or `channels`.
It applies the same channel rule and exits 0 with a JSON receipt on stdout, 1
on a Slack or API error, 2 on missing credentials or bad arguments, and 3 on an
unallowlisted or non-member channel. Capture that JSON without shell tracing or
verbose HTTP output. This is for proxy-less execution, not a way around the hub
from a connected worker.

## Verify without leaking

- Count tools, do not trust status. An authenticated `tools/list` showing
  exactly `slack_list_channels`, `slack_post_message`, `slack_read_channel`,
  and `slack_upload_file` is the proof; `Connected` is not.
- A missing bearer must return HTTP 401 with no tool names. Separate an empty
  variable from a wrong one by recording header state only, as
  `Authorization: Bearer <set|unset>`.
- Load named variables from the credentials file by matching them in a read
  loop rather than sourcing the file, and never echo the result.
- If an authenticated request still returns 401 after a token is registered
  hub-side, redeploy the hub: an environment change does not alter an
  already-running deployment.

## Rotation

Rotating one client mints a replacement for that label, appends only its
plaintext `MECHA_SLACK_TOKEN_<LABEL>` to the credentials file, replaces the
single hub variable holding the `label:sha256` allowlist, and restarts just
that client. Rotating the bypass secret rewrites `MECHA_VERCEL_BYPASS` in the
credentials file and restarts the clients and the proxy. Never print the
platform API response or the selected value during either operation.

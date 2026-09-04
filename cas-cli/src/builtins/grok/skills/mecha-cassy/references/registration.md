# Register the MechaCassy hub

Every request carries two headers: `Authorization: Bearer <per-client token>`,
issued by the hub for a client label and stored hub-side as a hash so labels
revoke individually, and `x-vercel-protection-bypass: <secret>`, checked at
the edge.
Both values live in the machine's credentials file as
`MECHA_SLACK_TOKEN_<LABEL>` and `MECHA_VERCEL_BYPASS` and are exported into the
environment by the login shell. Configurations below name those variables and
never hold their values.

## One command, once per machine

After `cas login`, `cas integrate mecha-cassy` writes all three registrations
below — a machine-scoped proxy registration under the user config directory
that every project inherits, plus the Codex and Claude Code entries — refusing
to claim success without an authenticated `tools/list` receipt. Re-running it
is the refresh path, and `cas doctor`'s `mecha-cassy` row states whether this
machine can post and what to do when it cannot. Setting up a new machine or a
teammate: `docs/MECHA_CASSY_ONBOARDING.md`.

The command also repairs a project `.cas/proxy.toml` that shadows the machine
registration: a project file **replaces** the machine allowlist rather than
widening it, so one left naming retired routes keeps them authoritative no
matter how often the machine file is rewritten. Where such a file already names
hub routes, the command corrects them in place — comments, key order, and every
unrelated server and route survive. It removes the file's own
`[servers.mecha-cassy]` block only when that block is identical to the machine
registration which supplies it; a block that differs is an override, such as a
project aimed at a staging hub, and is kept and named in the receipt rather
than silently switched. A project file that names *no* hub route is left alone,
because widening a policy the project declared is not this command's call;
`cas doctor` names that file and the exact routes to add.

The default client label is the uppercased hostname with non-alphanumeric
characters folded to `_`. `--label <MACHINE>` is only an override. The command
sends the existing Cassy Cloud bearer to `POST /api/clients` with
`{"label":"…","connector":"slack"}`. A `409 label_taken` retries once with
the first six characters of `~/.config/cas/device.json`'s device ID appended.
The optional bypass in the create response is used first; otherwise
`GET /api/bypass`, a read-only Vercel lookup, and one hidden prompt are tried
in that order. The Vercel PATCH endpoint is never used because it rotates the
shared secret. If `POST /api/clients` is absent, setup fails closed naming
`mecha-cassy#5` and never mints locally.

The hand-written shapes below remain the reference for repairing a machine by
hand or for a project that has never named the hub routes itself.

## Cassy proxy — reaches every harness

`.cas/proxy.toml`:

```toml
allowlist = [
  "mecha-cassy.mecha_read",
  "mecha-cassy.mecha_post",
]

[servers.mecha-cassy]
transport = "http"
url = "https://mecha-cassy.vercel.app/mcp/slack"
auth = "env:MECHA_SLACK_TOKEN_<LABEL>"

[servers.mecha-cassy.headers]
x-vercel-protection-bypass = "env:MECHA_VERCEL_BYPASS"
```

Dispatch through the proxy:

```text
cas__mcp_execute server=mecha-cassy tool=mecha_read args={"channel":"<name>","since":"<RFC3339>","max_messages":50}
```

The proxy resolves its bearer when `cas serve` starts, so a variable exported
after startup stays invisible until the next restart. `cas__system
action=proxy_health` is credential-free: the healthy record for `mecha-cassy`
reports `tool_count=2` and no error code. `.cas/proxy_catalog.json` is a
generated cache, not source configuration.

## Codex

`config.toml` under the Codex home:

```toml
[mcp_servers.mecha-cassy]
url = "https://mecha-cassy.vercel.app/mcp/slack"
bearer_token_env_var = "MECHA_SLACK_TOKEN_<LABEL>"
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
project's `scripts/slack-post.sh` with `post`, `upload`, or `read`.
It applies the same channel rule and exits 0 with a JSON receipt on stdout, 1
on a Slack or API error, 2 on missing credentials or bad arguments, and 3 on an
unallowlisted or non-member channel. Capture that JSON without shell tracing or
verbose HTTP output. This is for proxy-less execution, not a way around the hub
from a connected worker.

## Verify without leaking

- Count tools, do not trust status. An authenticated `tools/list` showing
  exactly `mecha_read` and `mecha_post` is the proof; `Connected` is not.
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

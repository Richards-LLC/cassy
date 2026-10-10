# Register the Violet hub

Every request carries two headers: `Authorization: Bearer <per-client token>`,
issued by the hub for a client label and stored hub-side as a hash so labels
revoke individually, and `x-vercel-protection-bypass: <secret>`, checked at
the edge.
Both values live in the machine's credentials file as
`VIOLET_SLACK_TOKEN_<LABEL>` and `VIOLET_VERCEL_BYPASS` and are exported into the
environment by the login shell. Configurations below name those variables and
never hold their values. `cas integrate violet` renames an existing machine's
legacy credential keys to these names and rewrites its registrations to match;
until it runs, credential lookup falls back to the legacy variables for one
release. Use the env names the integration receipt prints.

## One command, once per machine

After `cas login`, `cas integrate violet` writes all three registrations
below — a machine-scoped proxy registration under the user config directory
that every project inherits, plus the Codex and Claude Code entries — refusing
to claim success without an authenticated `tools/list` receipt. Re-running it
is the refresh path, and `cas doctor`'s `violet` row states whether this
machine can post and what to do when it cannot. Setting up a new machine or a
teammate: run `cas integrate violet --help` on that machine.

The command also repairs a project `.cas/proxy.toml` that shadows the machine
registration: a project file **replaces** the machine allowlist rather than
widening it, so one left naming retired routes keeps them authoritative no
matter how often the machine file is rewritten. Where such a file already names
hub routes, the command corrects them in place — comments, key order, and every
unrelated server and route survive. It removes the file's own
`[servers.violet]` block when that block points at the same hub URL as the
machine registration, which supplies it: a committed file cannot name a
per-machine hub client token. A block at a different URL is an override, such
as a project aimed at a staging hub. It is kept and named in the receipt rather
than silently switched, and a token it names that is missing is reported
against that file, because Cassy does not mint it. A project file that names
*no* hub route is left alone, because widening a policy the project declared is
not this command's call; `cas doctor` names that file and the exact routes to add.

Every Claude Code profile on the machine that already registers `violet` is
reconciled, not only the one `CLAUDE_CONFIG_DIR` selects: a literal bearer or
`MECHA_*` references are rewritten as env references, and profiles without a
`violet` entry are untouched. Each claude-code line in the receipt carries the
authenticated `tools/list` verdict, so "already current" never hides a rejected
bearer. Signing in again does not replace a hub client token the hub rejects:
unset the variable, delete its line from the credentials file, and re-run
`cas integrate violet` to mint a new client.

The default client label is the uppercased hostname with non-alphanumeric
characters folded to `_`. `--label <MACHINE>` is only an override. The command
sends the existing Cassy Cloud bearer to `POST /api/clients` with
`{"label":"…","connector":"slack"}`. A `409 label_taken` retries once with
the first six characters of `~/.config/cas/device.json`'s device ID appended.
The optional bypass in the create response is used first; otherwise
`GET /api/bypass`, a read-only Vercel lookup, and one hidden prompt are tried
in that order. The Vercel PATCH endpoint is never used because it rotates the
shared secret. If `POST /api/clients` is absent, setup fails closed naming
the Violet tracker issue (`cas config get issues.components.violet`) and never
mints locally.

The hand-written shapes below remain the reference for repairing a machine by
hand or for a project that has never named the hub routes itself.

Use the endpoint printed by `cas integrate violet` for `<HUB_MCP_URL>` in the repair examples below.

## Cassy proxy — reaches every harness

`.cas/proxy.toml`:

```toml
allowlist = [
  "violet.violet_read",
  "violet.violet_post",
]

[servers.violet]
transport = "http"
url = "<HUB_MCP_URL>"
auth = "env:VIOLET_SLACK_TOKEN_<LABEL>"

[servers.violet.headers]
x-vercel-protection-bypass = "env:VIOLET_VERCEL_BYPASS"
```

Dispatch through the proxy. `mcp_execute` takes a single `code` string holding
the JSON dispatch; it has no `server`, `tool` or `args` parameters:

```text
mcp_execute code='{"server":"violet","tool":"violet_read","args":{"channel":"<name>","since":"<RFC3339>","max_messages":50,"include_channels":false}}'
```

A project `allowlist` replaces the machine allowlist entirely, so list every
route the project needs (for example `"neon.*"`) alongside the Violet
routes. Prefix an entry with `supervisor:` (for example
`"supervisor:neon.*"`) to admit it for supervisors and plain sessions only;
factory workers are refused with a named reason. `cas serve` logs one
`callable tools: [...]` line at startup and after each reload, and
`mcp_search` marks such tools "supervisors only".

The proxy resolves its bearer when `cas serve` starts, so a variable exported
after startup stays invisible until the next restart. `system
action=proxy_health` is credential-free: the healthy record for `violet`
has no error code; discovery must include the two primary tools, even when
the upstream also advertises deprecated aliases. `.cas/proxy_catalog.json` is a
generated cache, not source configuration.

### Downstream projects and workers

Run `cas integrate violet` from the downstream checkout as well as on the
machine. A checkout with no `.cas/proxy.toml` inherits the machine-level hub
server and allowlist; a checkout with its own file uses that file's allowlist
as the dispatch policy. If `mcp_search` for `server:violet` returns no
tools while the machine registration is healthy, run `cas doctor` in that
checkout and add the exact `violet.violet_read` and
`violet.violet_post` entries it prints before restarting `cas serve`.
Do not copy a token into the project file. Verify the fresh worker sees both
tools before starting a release posting run.

## Codex

`config.toml` under the Codex home:

```toml
[mcp_servers.violet]
url = "<HUB_MCP_URL>"
bearer_token_env_var = "VIOLET_SLACK_TOKEN_<LABEL>"
env_http_headers = { "x-vercel-protection-bypass" = "VIOLET_VERCEL_BYPASS" }
```

`codex mcp list` must show `violet` enabled, naming the bearer variable
rather than a value.

## Claude Code

A user-scope HTTP server in the selected profile's `.claude.json`:

```json
{
  "mcpServers": {
    "violet": {
      "type": "http",
      "url": "<HUB_MCP_URL>",
      "headers": {
        "Authorization": "Bearer ${VIOLET_SLACK_TOKEN_<LABEL>}",
        "x-vercel-protection-bypass": "${VIOLET_VERCEL_BYPASS}"
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

A bounded `codex exec` or `claude -p` process with no live proxy runs
`cas violet post|thread|read`. The command connects with this machine's
`[servers.violet]` registration and credentials (the project's
`.cas/proxy.toml` and worker policy apply inside a Cassy project), prints the
hub's receipt, and exits non-zero on `ok: false`. Add `--json` to capture the
receipt as one JSON document; never add shell tracing or verbose HTTP output.
It is also the route for a local file from any session:
`cas violet post --channel <name> --file <path>` reads the bytes from disk, so
no base64 passes through the model. Codes are in
[contract.md](contract.md#posting-local-files-with-cas-violet).

## Verify without leaking

- Check tool names, not connection status. An authenticated `tools/list` showing
  `violet_read` and `violet_post` is the proof; deprecated aliases may also
  appear; `Connected` is not.
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
plaintext `VIOLET_SLACK_TOKEN_<LABEL>` to the credentials file, replaces the
single hub variable holding the `label:sha256` allowlist, and restarts just
that client. Rotating the bypass secret rewrites `VIOLET_VERCEL_BYPASS` in the
credentials file and restarts the clients and the proxy. Never print the
platform API response or the selected value during either operation.

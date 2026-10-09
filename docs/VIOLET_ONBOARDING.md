# Violet onboarding — one command per machine

Violet is the hosted hub that holds the Slack bot credential. Cassy sends
your **per-machine bearer** to the hub, and the hub talks to Slack. The bearer
is kept in the private machine credentials file so every local harness can
reuse the registration without copying a Slack bot token into a project.

Two values make a machine work, and they are the only two secrets involved:

| Variable | What it is | Who issues it |
| --- | --- | --- |
| `VIOLET_SLACK_TOKEN_<LABEL>` | Your machine's bearer. Per-machine so one can be revoked without touching anyone else. | `POST /api/clients`, authorized by your Cassy Cloud login |
| `VIOLET_VERCEL_BYPASS` | The shared edge-protection secret in front of the hub. | The hub's bypass route, Vercel read, or one hidden prompt |

The hub endpoint is `https://violet-hub.vercel.app/mcp/slack`.
Violet variables take precedence over installed-machine legacy variables;
empty Violet values also fall back. That fallback lasts one release. The shared
[compatibility manifest](../crates/cas-types/src/violet-compatibility.json)
defines the endpoint, its former hostname and the fallback names once for the
Rust runtime and release scripts. Only `cas integrate violet` is accepted.

`cas update --sync` (and `cas sync agents-md`) retire production hub entries
from project/user proxy files and all installed Claude/Codex account profiles,
and move them to the endpoint above. Proxy files also switch to the `VIOLET_*`
credential names, which resolve through the fallback until the credentials
file is renamed. Unrelated settings and custom endpoints are preserved.
`cas integrate violet` additionally renames the legacy keys in the credentials
file to `VIOLET_SLACK_TOKEN_<LABEL>` / `VIOLET_VERCEL_BYPASS` (values are
never printed) and rewrites the Claude and Codex entries to those names; open a
new login shell afterwards. Managed retired skill copies are removed;
operator-owned skills are retained.

Everything Cassy writes references those by **name**. No file in any repo, and
no line of terminal output, ever holds a value.

---

## For an operator: log in, then run one command

### 1. Use the existing Cassy Cloud login

The command uses team membership from the current `cas login` session. There
is no Violet admin token and no `VIOLET_ADMIN_TOKEN` setting.

```bash
cas login
cas integrate violet
```

The default label is the uppercased hostname with non-alphanumeric characters
folded to `_` (`soundwave` becomes `SOUNDWAVE`). Use `--label` only when that
hostname-derived label should be overridden:

```bash
cas integrate violet --label DANIEL_LAPTOP
```

The command calls `POST /api/clients` with the Cassy Cloud bearer. If the hub
returns `401` or `403`, the Cassy Cloud login is not authorized for the hub. If
it returns `409 {"error":"label_taken"}`, it retries once with the label plus
`_` and the first six characters of `~/.config/cas/device.json`'s device ID.
The token and bypass are written to `~/.config/cas/credentials.env` with
`0600` permissions, and the active login-shell profile sources that file.

If the create response has no bypass, Cassy reads the live `GET /api/bypass`
route with the same Cloud bearer. A Cloud outage is reported as HTTP `503`
with `{"error":"cloud_unavailable"}`; this is not a local token-minting
fallback. When the bypass route is unavailable for another reason, Cassy
performs a read-only Vercel lookup, then falls back to one hidden bypass prompt.
It never uses the Vercel PATCH endpoint, which rotates the shared secret.

Start a new shell after onboarding so an already-running client or `cas serve`
inherits the exported values.

That writes the machine-scoped registration to
`~/.config/code-mode-mcp/config.toml`, adds the Claude Code and Codex MCP
entries, and finishes with an authenticated `tools/list` printed as the
receipt. Every project on the machine inherits it — there is no per-project
file to copy.

Confirm:

```bash
cas doctor
```

The `violet` row under **Integrations** should be green and name the
tools the hub answered with.

---

## What the command actually does

| Artifact | Path | Contents |
| --- | --- | --- |
| Credentials | `~/.config/cas/credentials.env` | The two plaintext values, mode `0600`; unrelated exports are preserved |
| Login profile | `~/.profile`, `~/.bash_profile`, or `~/.zprofile` | A guarded source line for the credentials file |
| Machine proxy registration | `~/.config/code-mode-mcp/config.toml` | The hub URL, `auth = "env:VIOLET_SLACK_TOKEN_<LABEL>"`, the bypass header as `env:VIOLET_VERCEL_BYPASS`, and the allowlist of `violet_read` / `violet_post` plus the compatibility `violet` registration |
| Claude Code | `$CLAUDE_CONFIG_DIR/.claude.json` (else `~/.claude.json`) | A user-scope `http` server whose headers use `${VAR}`, expanded by the client at launch |
| Codex | `$CODEX_HOME/config.toml` (else `~/.codex/config.toml`) | `[mcp_servers.violet]` with `bearer_token_env_var` and `env_http_headers` |

Every write is idempotent — re-running the command *is* the refresh path — and
every unrelated key, comment, and table in those harness files is preserved.

Useful flags:

- `--label LABEL` — overrides the hostname-derived label and derives `VIOLET_SLACK_TOKEN_<LABEL>`.
- `--token-env NAME --bypass-env NAME` — override the variable names. If the
  hub cannot return a bypass, the command may use the one hidden prompt.
- `--no-harness` — write only the machine registration, leave Claude Code and
  Codex alone.
- `--dry-run` — report every planned change, write nothing.
- `--skip-verify` — skip the hub round-trip when setting up offline. The
  doctor row then stays amber until something has actually verified.
- `--json` — print exactly one `VioletReport` JSON document on stdout instead
  of the human summary. The report includes the hub URL, credential variable
  names and states, planned or completed writes, probe result and remedy.
  A rejected probe still emits its report, exits nonzero and explains the
  refusal on stderr. Failures before report creation emit no report.

---

## Reading a red `violet` row

| Row says | What happened | Fix |
| --- | --- | --- |
| `not registered on this machine` | No hub server in the machine config. | `cas integrate violet` |
| `VIOLET_SLACK_TOKEN_… is unset` / `set but empty` | The registration is fine; the credentials file is not. | Add the value, **open a new shell** |
| `hub rejected this machine (HTTP 401…)` | The bearer is not registered hub-side, was revoked, or the hub was not redeployed after the token was added. | Confirm `cas login`, then re-run the command; the route names the cloud-login failure |
| `hub tool contract drifted…` | The hub renamed or added tools; the allowlist names the old ones, so every call would be denied by policy. | `cas integrate violet` rewrites the allowlist. The row names the file the stale entries are in — machine or project — and the command rewrites that same file |
| `…is authoritative for dispatch policy and names none` | This project has its own `.cas/proxy.toml`, and a project file **replaces** the machine allowlist rather than widening it. | Add the hub routes to that project's `allowlist`, exactly as the message spells them |

That last one is deliberate, not a bug: a machine-wide policy must never
silently widen what a project has declared it will dispatch. A project file
that already names hub routes is a different case — there the command *does*
rewrite them, because correcting a route the project itself asked for is not
widening its policy.

---

## Hub deployment contract

The hub repository is [`Richards-LLC/violet_ps`](https://github.com/Richards-LLC/violet_ps). The client contract is:

- `POST /api/clients` accepts `Authorization: Bearer <Cassy Cloud token>` and
  `{"label":"…","connector":"slack"}`; it returns a bearer once, rejects
  unauthorized callers with `401` or `403`, and reports a duplicate label as
  `409 {"error":"label_taken"}`.
- `GET /api/bypass` accepts the same authorization and returns the existing
  bypass value; Cloud outages return `503 {"error":"cloud_unavailable"}`.
- `DELETE /api/clients/<label>` revokes that machine label and its bearer.
- `VIOLET_CLOUD_TEAMS` is the hub's allowlist of Cassy Cloud team slugs;
  production includes `petra-stella`.

The hub should generate a random secret, append its `label:sha256` pair to the
allowlist variable, and **redeploy**. An environment change does not alter an
already-running deployment.

Send the teammate the plaintext once, out of band. It is stored hub-side as a
hash, so it cannot be recovered later; a lost token is re-minted, not looked
up. Revoking one machine means removing its single `label:sha256` pair — no
other machine is disturbed.

Rotating `VIOLET_VERCEL_BYPASS` affects every machine at once: rotate it, then
have everyone update their credentials file and restart their clients and
`cas serve`.

---

## Consumer test channel

Use `#cas-scratch` (`C0BUZEB4H3M`) for consumer validation of the hub tools. Keep
test posts there rather than in release or internal announcement channels.

---

## Related

- `cas-cli/src/builtins/skills/violet/SKILL.md` — how to *post* once this
  is green (channel rules, thread order, receipts).
- `cas-cli/src/builtins/skills/violet/references/registration.md` — the
  per-file registration shapes, for a machine being repaired by hand.
- `docs/RELEASE_SLACK_RUBRIC.md` — what to post and where.

# MechaCassy onboarding — one command per machine

MechaCassy is the hosted hub that holds the Slack bot credential. Cassy sends
your **per-machine bearer** to the hub, and the hub talks to Slack. The bearer
is kept in the private machine credentials file so every local harness can
reuse the registration without copying a Slack bot token into a project.

Two values make a machine work, and they are the only two secrets involved:

| Variable | What it is | Who issues it |
|---|---|---|
| `MECHA_SLACK_TOKEN_<LABEL>` | Your machine's bearer. Per-machine so one can be revoked without touching anyone else. | `POST /api/clients`, authorized by your Cassy Cloud login |
| `MECHA_VERCEL_BYPASS` | The shared edge-protection secret in front of the hub. | The hub's bypass route, Vercel read, or one hidden prompt |

Everything Cassy writes references those by **name**. No file in any repo, and
no line of terminal output, ever holds a value.

---

## For an operator: log in, then run one command

### 1. Use the existing Cassy Cloud login

The command uses team membership from the current `cas login` session. There
is no MechaCassy admin token and no `MECHA_ADMIN_TOKEN` setting.

```bash
cas login
cas integrate mecha-cassy
```

The default label is the uppercased hostname with non-alphanumeric characters
folded to `_` (`soundwave` becomes `SOUNDWAVE`). Use `--label` only when that
hostname-derived label should be overridden:

```bash
cas integrate mecha-cassy --label DANIEL_LAPTOP
```

The command calls `POST /api/clients` with the Cassy Cloud bearer. If the hub
returns `409 {"error":"label_taken"}`, it retries once with the label plus
`_` and the first six characters of `~/.config/cas/device.json`'s device ID.
The token and bypass are written to `~/.config/cas/credentials.env` with
`0600` permissions, and the active login-shell profile sources that file.

If the create response has no bypass, Cassy reads `GET /api/bypass`. When that
route is unavailable it performs a read-only Vercel lookup, then falls back to
one hidden bypass prompt. It never uses the Vercel PATCH endpoint, which rotates
the shared secret.

Both hub routes are currently absent. Until `mecha-cassy#5` is deployed, the
command fails closed with the route name; it never mints a token locally.

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

The `mecha-cassy` row under **Integrations** should be green and name the
tools the hub answered with.

---

## What the command actually does

| Artifact | Path | Contents |
|---|---|---|
| Credentials | `~/.config/cas/credentials.env` | The two plaintext values, mode `0600`; unrelated exports are preserved |
| Login profile | `~/.profile`, `~/.bash_profile`, or `~/.zprofile` | A guarded source line for the credentials file |
| Machine proxy registration | `~/.config/code-mode-mcp/config.toml` | The hub URL, `auth = "env:MECHA_SLACK_TOKEN_<LABEL>"`, the bypass header as `env:MECHA_VERCEL_BYPASS`, and the allowlist of the hub's current tools |
| Claude Code | `$CLAUDE_CONFIG_DIR/.claude.json` (else `~/.claude.json`) | A user-scope `http` server whose headers use `${VAR}`, expanded by the client at launch |
| Codex | `$CODEX_HOME/config.toml` (else `~/.codex/config.toml`) | `[mcp_servers.mecha-cassy]` with `bearer_token_env_var` and `env_http_headers` |

Every write is idempotent — re-running the command *is* the refresh path — and
every unrelated key, comment, and table in those harness files is preserved.

Useful flags:

- `--label LABEL` — overrides the hostname-derived label and derives `MECHA_SLACK_TOKEN_<LABEL>`.
- `--token-env NAME --bypass-env NAME` — override the variable names. If the
  hub cannot return a bypass, the command may use the one hidden prompt.
- `--no-harness` — write only the machine registration, leave Claude Code and
  Codex alone.
- `--dry-run` — report every planned change, write nothing.
- `--skip-verify` — skip the hub round-trip when setting up offline. The
  doctor row then stays amber until something has actually verified.

---

## Reading a red `mecha-cassy` row

| Row says | What happened | Fix |
|---|---|---|
| `not registered on this machine` | No hub server in the machine config. | `cas integrate mecha-cassy` |
| `MECHA_SLACK_TOKEN_… is unset` / `set but empty` | The registration is fine; the credentials file is not. | Add the value, **open a new shell** |
| `hub rejected this machine (HTTP 401…)` | The bearer is not registered hub-side, was revoked, or the hub was not redeployed after the token was added. | Confirm `cas login`, then re-run the command; the route names the cloud-login failure |
| `hub tool contract drifted…` | The hub renamed or added tools; the allowlist names the old ones, so every call would be denied by policy. | `cas integrate mecha-cassy` rewrites the allowlist. The row names the file the stale entries are in — machine or project — and the command rewrites that same file |
| `…is authoritative for dispatch policy and names none` | This project has its own `.cas/proxy.toml`, and a project file **replaces** the machine allowlist rather than widening it. | Add the hub routes to that project's `allowlist`, exactly as the message spells them |

That last one is deliberate, not a bug: a machine-wide policy must never
silently widen what a project has declared it will dispatch. A project file
that already names hub routes is a different case — there the command *does*
rewrite them, because correcting a route the project itself asked for is not
widening its policy.

---

## Hub deployment contract

The hub lives in `petra_stella_tools/mecha_cassy`. The client contract is:

- `POST /api/clients` accepts `Authorization: Bearer <Cassy Cloud token>` and
  `{"label":"…","connector":"slack"}`; it returns a bearer once.
- `GET /api/bypass` accepts the same authorization and returns the existing
  bypass value.
- Missing routes must remain a hard failure for token creation until
  `mecha-cassy#5` is deployed.

The hub should generate a random secret, append its `label:sha256` pair to the
allowlist variable, and **redeploy**. An environment change does not alter an
already-running deployment.

Send the teammate the plaintext once, out of band. It is stored hub-side as a
hash, so it cannot be recovered later; a lost token is re-minted, not looked
up. Revoking one machine means removing its single `label:sha256` pair — no
other machine is disturbed.

Rotating `MECHA_VERCEL_BYPASS` affects every machine at once: rotate it, then
have everyone update their credentials file and restart their clients and
`cas serve`.

---

## Related

- `cas-cli/src/builtins/skills/mecha-cassy/SKILL.md` — how to *post* once this
  is green (channel rules, thread order, receipts).
- `cas-cli/src/builtins/skills/mecha-cassy/references/registration.md` — the
  per-file registration shapes, for a machine being repaired by hand.
- `docs/RELEASE_SLACK_RUBRIC.md` — what to post and where.

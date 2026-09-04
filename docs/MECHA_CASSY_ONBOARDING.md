# MechaCassy onboarding — a new machine, a new teammate

MechaCassy is the hosted hub that holds the Slack bot credential. Nothing on
your machine ever sees a Slack token: Cassy sends your **per-machine bearer**
to the hub, the hub talks to Slack.

Two values make a machine work, and they are the only two secrets involved:

| Variable | What it is | Who issues it |
|---|---|---|
| `MECHA_SLACK_TOKEN_<LABEL>` | Your machine's bearer. Per-machine so one can be revoked without touching anyone else. | The hub admin |
| `MECHA_VERCEL_BYPASS` | The shared edge-protection secret in front of the hub. | The hub admin |

Everything Cassy writes references those by **name**. No file in any repo, and
no line of terminal output, ever holds a value.

---

## For a teammate: three steps to green

### 1. Ask the hub admin for a label and a token

Tell them a label for this machine — your name plus the device is a good one
(`DANIEL_LAPTOP`, `SAM_DESKTOP`, `CI_RUNNER`). They mint a bearer for exactly
that label and send you two values: `MECHA_SLACK_TOKEN_<LABEL>` and
`MECHA_VERCEL_BYPASS`.

### 2. Put both values in your credentials file

They belong in the file your login shell exports, so every terminal and every
harness inherits them. If you do not have one yet:

```bash
scripts/mecha-cassy-credentials.sh
```

That wizard is the only step a human does by hand. It prompts for the two
values without echoing them, writes `~/.config/cas/credentials.env` with `0600`
permissions, and tells you the one line to add to your shell profile. It never
prints a value back and never sends one anywhere.

Then **start a new shell**. A variable exported inside a running session is
invisible to processes that were already started, including a running
`cas serve`.

### 3. Run one command

```bash
cas integrate mecha-cassy --label DANIEL_LAPTOP
```

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
| Machine proxy registration | `~/.config/code-mode-mcp/config.toml` | The hub URL, `auth = "env:MECHA_SLACK_TOKEN_<LABEL>"`, the bypass header as `env:MECHA_VERCEL_BYPASS`, and the allowlist of the hub's current tools |
| Claude Code | `$CLAUDE_CONFIG_DIR/.claude.json` (else `~/.claude.json`) | A user-scope `http` server whose headers use `${VAR}`, expanded by the client at launch |
| Codex | `$CODEX_HOME/config.toml` (else `~/.codex/config.toml`) | `[mcp_servers.mecha-cassy]` with `bearer_token_env_var` and `env_http_headers` |

Every write is idempotent — re-running the command *is* the refresh path — and
every unrelated key, comment, and table in those harness files is preserved.

Useful flags:

- `--label LABEL` — derives `MECHA_SLACK_TOKEN_<LABEL>`.
- `--token-env NAME --bypass-env NAME` — name the variables outright; fully
  non-interactive, nothing is prompted.
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
| `hub rejected this machine (HTTP 401…)` | The bearer is not registered hub-side, was revoked, or the hub was not redeployed after the token was added. | Ask the admin to re-mint or redeploy, re-export, re-run the command |
| `hub tool contract drifted…` | The hub renamed or added tools; the allowlist names the old ones, so every call would be denied by policy. | `cas integrate mecha-cassy` rewrites the allowlist |
| `…is authoritative for dispatch policy and names none` | This project has its own `.cas/proxy.toml`, and a project file **replaces** the machine allowlist rather than widening it. | Add the hub routes to that project's `allowlist`, exactly as the message spells them |

That last one is deliberate, not a bug: a machine-wide policy must never
silently widen what a project has declared it will dispatch.

---

## For the hub admin: minting a machine token

The hub lives in `petra_stella_tools/mecha_cassy`. Minting a bearer for a new
label means generating a random secret, appending its `label:sha256` pair to
the hub's allowlist variable, and **redeploying** — an environment change does
not alter an already-running deployment, so a token added without a redeploy
returns 401 to a correctly configured machine and looks like the teammate's
fault.

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

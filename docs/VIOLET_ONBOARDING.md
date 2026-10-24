# Violet onboarding — one command per machine

`cas update` migrates existing Violet registrations in every installed Claude
and Codex account profile, including `CLAUDE_CONFIG_DIR` and `CODEX_HOME`.
It renames legacy credential exports and references, then reports an
authenticated `tools/list` result for each production-hub profile. A missing,
empty or ambiguous credential produces a named advisory; builtin sync continues
and no empty bearer is sent. Custom endpoints and working
account-specific credentials are preserved. No manual integration step is
needed after an upgrade. Cassy's Claude/Codex CLI and factory launch paths load
missing or empty canonical credentials referenced by that harness from the private
credentials file directly. Factory workers receive the union of native registration
references and explicit proxy/config grants, subject to server and environment denials;
update does not edit shell profiles. Only an unavailable retired managed token
name may move to a different label, and only when exactly one usable canonical
token exists. Update also repairs an unavailable
`env:VIOLET_SLACK_TOKEN_CASSY_PROXY` in project and user proxy files, including
unmarked files, when exactly one account bearer reference is registered in an
active or explicitly configured production Claude/Codex profile and its
credential is usable. Backup profile directories and exported tokens alone
cannot select an account. Full update repairs account registrations before
refreshing project proxies, so one run converges. Multiple registered accounts
refuse the write with `ambiguous_violet_credential`; absent accounts, malformed,
symlinked or foreign-owned proxy files produce update warnings while rules
and skills continue syncing. Doctor retains its normal health result and
severity, adding repair advisories only for stale defaults; a malformed proxy
remains an error, and a working custom symlink still reaches the hub probe. Custom refs, custom endpoints, working
defaults (including managed runtime selection), comments and dispatch policy
are preserved. The persisted repair serves both
`cas violet` and the `cas serve` upstream used by MCP artifact posting. Every
mapping prints variable names only. `PASS (Cassy launches)` covers `cas claude` and `cas codex`; the
receipt names references absent from the updater's exported environment. Export
those names before using bare `claude` or `codex`.

The user proxy is shared across profiles. Repair binds its explicit bearer
reference to the unique account visible in the updater's active or configured
profiles, and that account then applies to every profile inheriting the user
proxy. Switching shells or profiles later does not retarget the saved reference.
Update prints this shared scope alongside the variable-name mapping. Project
registrations continue to take precedence over the shared user registration.

The private file must be a
regular file owned by the current user, have no group/other permissions, and be
at most 1 MiB; symlinks are refused. CLI launches and update print the refused path
and the ownership/`chmod 600` remedy; update leaves an unsafe file untouched.
Probes share that launch resolver and have a 30-second total budget (at most
10 seconds per profile).

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

## Factory channel watches

From 3.59, factory sessions on the same clone share
`.cas/violet/watches.v2.json` (format version 2), with separate watch entries for
each session and channel. Each mutation reloads state under a shared lock.
Starting a second session leaves live peer watches intact. Clean shutdown stops
its own watches without waiting for an in-flight Cloud request; that tick
finishes retirement. The last runtime handle also retires watches on early
returns. After a successful session listing, a deleted metadata receipt
also retires the old session's watches; unreadable listings or an existing but
incomplete receipt do not prove a live daemon is gone.

The first authenticated use migrates `violet/watches.json` into the v2 book
without changing the legacy file. Doctor and factory status can read legacy
health until the v2 file exists, without writing or migrating either file. A
present but invalid or unsupported v2 book never falls back to legacy health.
Later daemon starts use only the v2 book. Daemons
from 3.58 and older keep their own legacy watches until upgraded; upgrade and
restart every daemon on a clone for coordinated watch behavior. Older binaries
cannot overwrite the v2 path, and v2 readers refuse future format versions.

Activity on a thread already recorded in a watch goes to that watch's owning
session, even when another session claims it from Cloud. The first durable wake
on an unknown thread claims that root under the lock, so later readers and
replies use one owner. This is local watch routing; it does not add outgoing-post
tracking or Cloud thread claims.

After enqueueing a known owner's reply, the claimant may refresh that watch's
activity timestamp and bounded dedupe keys for that thread under the same lock.
Each thread keeps up to 200 peer-admitted keys, matching the normal dedupe
window. A busy thread cannot evict the owner's keys for other threads. The owner
keeps at most 200 recent thread roots per channel, discarding the oldest roots
and their peer keys together; activity on an older root then follows the
unknown-thread rule. Stopping a watch clears its roots and peer keys, and
reopening starts a fresh ownership set. Admission does
not start or stop the owner's watches, alter other watches, or advance the channel-wide
read cursor past unread messages on other threads.

Without Cloud credentials, ticks create no watch book or claim-failure finding.
An enabled consumer checks credentials on its 15-second tick; after `cas login`
the factory daemon starts receiving without a restart.

A corrupt native book is durably quarantined as `watches.v2.json.corrupt`
(numbered if necessary), then replaced with fresh state. Doctor names the
quarantine. Inspect and archive or remove that file after review to clear the
finding. A corrupt legacy file is left unchanged, with a quarantine copy;
recovery never writes an older daemon's file.

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

## Inspect project channel bindings

Declare every desired channel, including channels that are not mapped yet:

```bash
cas config set slack.channels '["C0123456789", "G0123456789"]'
cas violet channels
```

The report compares Violet's channel mappings with Cloud's binding verification.
It includes configured channels, Violet-only mappings, Cloud-only bindings and
previously gated channels. Project mismatches require inspection before remapping;
Cloud verification of a stored binding does not prove that the two stores agree.
Violet's listing is discovery; the server verifies mapping authority during repair.

Cloud-only discovery requires an existing supervisor registration for your account
and this project. From another terminal, pass its ID with
`cas violet channels --registration-id <supervisor-id>`. The command never creates
a registration. If that read fails or reaches Cloud's 100-binding limit, the
report explicitly marks its inventory incomplete while checking known channels.
Use `--json` for full identities and machine-readable flags. Findings or incomplete
inventory exit with status 1.

Follow each row's remedy, then run `cas update` for ordinary authorized repair.
Listing is read-only. Repair uses the existing local drain and Cloud reconciliation
contract; it does not require an administrative secret. Pre-binding history keeps
Cloud's archival policy and is not backfilled to later readers.

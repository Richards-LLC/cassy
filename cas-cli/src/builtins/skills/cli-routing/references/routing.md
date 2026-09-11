# One-shot CLI routing reference

## Capacity is runtime state

There is no reliable quota preflight for either CLI in this environment. Start
with Codex, and treat only a completed one-shot's nonzero exit plus its captured
stderr as capacity/auth evidence. We found no reproducible local Codex
out-of-tokens text and no reproducible Claude over-limit text; do **not** invent
either signature. Save the exact observed output before routing or reporting a
blocker.

## Codex first

[cas-codex-exec](../../cas-codex-exec/SKILL.md) owns the canonical `codex exec`
recipe — the invocation, the sandbox flag, the model default, closing stdin, and
redirecting long output to a file. Use it as written; do not restate it here.

What routing adds on top of that recipe: reads can stay sandboxed, but a write
needs a narrowly scoped prompt and
`--dangerously-bypass-approvals-and-sandbox` in an externally sandboxed
session. Prefer `-c model_reasoning_effort="low"` for mechanical or
transcription work. Keep the captured output file and its exit status; they are
the only admissible evidence if routing falls through to Claude.

For non-Slack plugin tools, use the session's tool-discovery surface; do not
infer availability from `~/.codex/config.toml`. This CLI fallback never
authorizes a personal Slack connector.

When using `--output-schema <FILE>`, the schema is strict: at **every** object
nesting level, `required` must list every key present in `properties`. Validate
the schema before the call; an optional-looking property omitted from `required`
is rejected by Codex's strict schema handling.

## Claude hard account gate

`claude -p` / `claude --print` is the one-shot mode. Before every Claude call,
probe the exact profile that would make the call:

```bash
CLAUDE_CONFIG_DIR="$HOME/.claude-alt" \
  claude auth status --json < /dev/null | jq '{loggedIn, authMethod, apiProvider, email, subscriptionType}'
```

Which accounts are approved is operator policy, held in configuration rather
than in this skill:

```bash
cas config get release.claude_account_allowlist
cas config set release.claude_account_allowlist "ops@example.com,release@example.com"
```

The gate passes only when the probe reports `loggedIn: true`, `authMethod:
"claude.ai"`, `apiProvider: "firstParty"`, and an address on that allowlist
(compared case-insensitively). A credential-free passing probe has this shape:

```json
{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"<allowlisted address>","subscriptionType":"max"}
```

The list is empty by default and the gate fails closed, so a project that has
not configured it approves no Claude account at all. An address outside the
allowlist is an unapproved account and hard-fails the gate even when
`loggedIn` is true. A missing profile, false `loggedIn`, wrong auth method or
provider, malformed JSON, or a failed probe also fails closed. Do not inspect
or copy credential tokens from config files; the status command is the account
authority.

Only after a passing probe may a one-shot run:

```bash
CLAUDE_CONFIG_DIR="$HOME/.claude-alt" \
  claude -p --output-format text "Perform only the bounded request described here." \
  < /dev/null > /tmp/claude-one-shot.out 2>&1
status=$?
```

If the probe is ambiguous or the call reports a rate/capacity failure, do not
try another Claude profile. Return to Codex when it remains usable; otherwise
report blocked with both captured receipts.

## Posting release notes

The trigger is automatic on every merge to `main` or `staging`. Use
[release-notes](../../release-notes/SKILL.md) and the project's rubric for
channel, message shape and ordering; a project with no rubric and no channel
posts nowhere.

Use only the MechaCassy hub/bot through
[mecha-cassy](../../mecha-cassy/SKILL.md). Never use Claude.ai Slack or a personal
connector; the non-Slack account gate above does not authorize Slack transport.
The hub skill owns authenticated `tools/list`, bounded `mecha_read` dedupe,
ordered `mecha_post` calls, and message/file integrity receipts. A one-shot
without a live proxy uses the same hub's registered direct route, as described
in its registration reference. If that route cannot complete publication,
save the draft and partial receipts and report the measured failure to the
supervisor; handoff does not change the transport.

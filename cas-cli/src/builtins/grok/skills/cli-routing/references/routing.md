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
(including a Slack post) needs a narrowly scoped prompt and
`--dangerously-bypass-approvals-and-sandbox` in an externally sandboxed
session. Prefer `-c model_reasoning_effort="low"` for mechanical or
transcription work. Keep the captured output file and its exit status; they are
the only admissible evidence if routing falls through to Claude.

Plugin-backed tools are not necessarily in Codex's own tool list. Discover
them through `list_mcp_resources` and look for the `codex_apps` resource (for
example Slack), rather than deciding a plugin is unavailable from its function
list or `~/.codex/config.toml`.

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

The trigger is automatic on every merge to `main` or `staging`. Everything
else — target channel, message shape, reply count, and which Slack transport is
approved — is project policy, owned by the project's release-notes rubric.
Use [release-notes](../../release-notes/SKILL.md) and that rubric for content;
a project with no rubric and no channel posts nowhere.

This skill contributes only the routing half of the sequence:

1. Read the target channel first to deduplicate a previous or partial attempt.
2. Choose the posting CLI by the gate above: Codex when its Slack plugin is
   present in the current session's resource probe, Claude only after its
   account gate passes. When neither is available, hand the drafted bodies to
   the supervisor rather than posting from an unapproved account.
3. Retain the returned message timestamps as receipts.

The rubric owns the exact commands, message bodies, and ordering; do not create
a competing copy here.

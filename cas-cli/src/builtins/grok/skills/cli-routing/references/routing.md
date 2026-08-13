# One-shot CLI routing reference

## Capacity is runtime state

There is no reliable quota preflight for either CLI in this environment. Start
with Codex, and treat only a completed one-shot's nonzero exit plus its captured
stderr as capacity/auth evidence. We found no reproducible local Codex
out-of-tokens text and no reproducible Claude over-limit text; do **not** invent
either signature. Save the exact observed output before routing or reporting a
blocker.

## Codex first

For read-only investigations, use [cas-codex-exec](../../cas-codex-exec/SKILL.md)
instead of duplicating its prompt and backgrounding guidance. For a bounded
one-shot, close stdin and redirect long output to a file; never pipe a long run
through `tail`:

```bash
codex exec --skip-git-repo-check --sandbox read-only \
  -c model_reasoning_effort="low" \
  "Inspect only the named files and return the requested summary." \
  < /dev/null > /tmp/codex-one-shot.out 2>&1
status=$?
```

Use `model_reasoning_effort="low"` for mechanical/transcription work. Reads
can stay sandboxed. Writes (including a Slack post) require a narrowly scoped
prompt and `--dangerously-bypass-approvals-and-sandbox` in an externally
sandboxed session. Read output from `/tmp/codex-one-shot.out`; retain it with
the `status` if routing falls through.

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

The probe is verified on this machine with Claude Code 2.1.231. The explicit
allowlist contains exactly one email: `pippenz@gmail.com`. The gate passes only
when JSON has `loggedIn: true`, `authMethod: "claude.ai"`, `apiProvider:
"firstParty"`, and that exact email. The live approved-profile probe on
2026-08-13 produced this decisive, credential-free shape:

```json
{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"pippenz@gmail.com","subscriptionType":"max"}
```

Any email outside that one-entry allowlist is an unapproved account and
hard-fails the gate even when `loggedIn` is true. A missing profile, false
`loggedIn`, wrong auth method or provider, malformed JSON, or a failed probe
also fails closed. Do not inspect or copy credential tokens from config files;
the status command is the account authority.

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

The trigger is automatic on every merge to `main` or `staging`; only the target
channel is project-specific. Use [release-notes](../../release-notes/SKILL.md)
for content and its project rubric. For cas-src, the channel is
`#cas-internal` (`C0B44GUKDK2`); local-only projects post nowhere.

Follow [`docs/SLACK_POSTING_RUNBOOK.md`](../../../../../../docs/SLACK_POSTING_RUNBOOK.md)
for transport, and [`docs/RELEASE_SLACK_RUBRIC.md`](../../../../../../docs/RELEASE_SLACK_RUBRIC.md)
for content. The operational sequence is:

1. Write the exact user and dev bodies to a file with `=== MESSAGE N ===`
   separators; keep the headers out of the sent text.
2. Read the target channel first to deduplicate a previous or partial attempt.
3. Use the Codex `codex_apps` Slack plugin through the Codex-first route; use
   Claude Slack MCP only if the already-gated current profile authorizes it.
4. Post the two top-level messages and required replies in rubric order, then
   record the returned message timestamps as receipts.

The runbook owns the exact Slack commands, stdin/approval/buffering traps, and
write permission details; do not create a competing copy here.

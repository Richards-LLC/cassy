---
name: cli-routing
description: Use when a bounded task needs a one-shot Codex or Claude CLI subprocess, including capacity recovery or release-note posting; Claude requires the explicitly approved pippenz@gmail.com profile.
managed_by: cas
---

# CLI Routing

Route one-shot work by the capacity actually available now, not by a presumed
account or quota. Use `codex exec` or `claude -p` only for a bounded
subprocess, not as a replacement for a factory worker or the current agent.

## Route

1. Try **Codex first**. Use [cas-codex-exec](../cas-codex-exec/SKILL.md) for
   read-only, token-heavy investigation; use the routing reference for a
   narrowly scoped write or structured-output call.
2. If Codex actually fails for capacity/auth, preserve its command, exit status,
   and stderr. Do not infer an exhaustion signature from slow output.
3. Claude is a fallback **only** after `claude auth status --json`, run with the
   exact `CLAUDE_CONFIG_DIR`, reports `loggedIn: true`, `authMethod:
   "claude.ai"`, `apiProvider: "firstParty"`, and the exact email
   `pippenz@gmail.com`. This is a one-entry allowlist: any other email is an
   unapproved account, and missing config, malformed JSON, or a failed probe is
   also a hard no-Claude result.
4. If eligible Claude also fails for capacity/rate limit, preserve its evidence
   and report blocked. Never spend another Claude account to work around it.

See [routing.md](references/routing.md) for runnable probes, one-shot examples,
strict Codex output schemas, and the verified local approved-account result.

## Release-note posting

Every merge to `main` or `staging` needs the existing
[release-notes](../release-notes/SKILL.md) flow; channel choice remains
project-local. For the actual Slack transport, follow
[`docs/SLACK_POSTING_RUNBOOK.md`](../../../../../docs/SLACK_POSTING_RUNBOOK.md):
write exact bodies to a separator-delimited file, read the channel to dedupe,
then post and retain returned timestamps as receipts. That flow uses this
skill's Codex-first router; Claude Slack MCP is allowed only on the eligible
current profile.

## Do not trigger

Do not use for ordinary coding, interactive CLI sessions, routine model choice,
or broad research already suited to a factory worker. Do not use Claude merely
because a profile directory exists: the account gate is mandatory every time.

# 2026-07-30 — Harness diary sweep — #cas-internal thread

## Parent post

🧭 The cross-harness compatibility picture is current through Grok 0.2.114, Claude Code 2.1.220, and Codex 0.146.0: the strongest changes improve tool evidence and connection recovery, while launch, environment, MCP, skills, and approval seams still deserve upgrade checks.

Claude brings mostly host-side reliability wins; Codex refreshes several live integration paths; and Grok's newest attributable notes expose useful changes alongside substantial upstream history gaps. This is a harness compatibility and watch-ledger update—not a Cassy runtime release.

## Grok reply

**Grok 0.2.107–0.2.114:** the installed version is 0.2.114, but only 0.2.112 has attributable local release notes. That evidence adds launch version policy, environment filtering for custom providers, native resume/transcript changes, live MCP enrollment updates, inherited MCP tools for plugin subagents, `config.toml` hooks, and more truthful background-command and transcript status.

**Verdict/action:** 👀 validate unattended launch, Cassy identity environment, UUID-based session evidence, persistent `cas__*` MCP discovery, and `--rules` precedence; no Cassy runtime change follows from this diary review. **Source gaps:** 0.2.107–0.2.111 and 0.2.113–0.2.114 have no attributable notes on the available local Markdown/JSON surfaces, so no release behavior or verdict was inferred for them.

## Claude reply

**Claude Code 2.1.218–2.1.220:** Opus 5 is now the default behind the stable `opus` alias, dynamic and nested delegation defaults changed while Cassy's review workflow remains explicitly bounded, and MCP startup diagnostics became clearer. Background built-in review stays separate from Cassy review authority, while fixes for dropped tool errors, unpaired tool blocks, and compacted fork lineage improve evidence integrity.

**Verdict/action:** 👀 watch deployments that rely on the moving `opus` alias; 🟢/✅ take the bounded-workflow, MCP-diagnostic, and evidence-correctness gains with no Cassy runtime change. **Source limitation:** Anthropic's 2.1.220 note is only a generic bug-fix and reliability rollup, so no component-specific impact is attributed to that version.

## Codex reply

**Codex stable 0.146.0:** live MCP reconciliation now refreshes authentication, configuration, connections, and Apps tools without restarting healthy servers. Executor-provided skills and resources, Agent Plugins, skill-catalog retention, approval continuity across interruptions and forks, and broader proxy routing all touch Cassy's MCP, mirrored-skills, multi-agent, sandbox, and network assumptions.

**Verdict/action:** 👀 smoke live `cs` MCP refresh and catalog freshness, confirm mirrored skills and agents remain visible without plugin shadowing, and verify resumed or forked sessions preserve non-interactive approval behavior. No Cassy runtime change was required. **Source gaps:** none—the stable entry is grounded in OpenAI's official 0.146.0 release body; 0.147.0 alphas remain outside stable tracking.

---
date: 2026-05-01
topic: codex-cli-empirical-reference
focus: what Codex CLI actually exposes in 2026 — extensibility surfaces, schemas, paths, ecosystem patterns
scope: Codex + Claude Code as the only two supported harnesses (no OpenCode, no Cursor)
related: 2026-04-28-cas-harness-portability-ideation.md (different second-harness scope — OpenCode)
---

# Codex CLI Empirical Reference (2026)

Empirical reference for the OpenAI Codex CLI as of 2026-05-01.

**Scope note:** The user has chosen **Codex + Claude Code** as the supported harnesses. Codex (not OpenCode) is the second harness. The 2026-04-28 ideation explored OpenCode as the second harness; its strategic ideas (manifest, conformance suite, hook taxonomy) inform this work but the specific OpenCode adapter is out of scope. Cursor is also out of scope.

## TL;DR

- Codex is structurally **far closer to Claude Code than the existing Cassy abstraction assumes**. Same SKILL.md format, same plugin manifest convention, hooks with identical wire format, first-class MCP, role-based subagents.
- Codex's own source ships `codex-rs/external-agent-sessions/` — a Claude Code session importer. Cassy doing the reverse direction is the symmetric problem with reference code already written.
- AGENTS.md was donated to the Linux Foundation Agentic AI Foundation in December 2025 (alongside MCP from Anthropic). It is now the cross-tool standard backed by Google, OpenAI, Cursor, Factory, Sourcegraph.
- Major real gap: Codex speaks **only the OpenAI Responses API on the wire**. Pointing it at Anthropic models requires a Responses-shimming proxy.
- The `cas-mux/harness.rs:26-31` capability matrix is stale — declares `supports_hooks: false` and `supports_subagents: false`, both wrong for current Codex.

---

## 1. Project & user instructions (CLAUDE.md analog)

| | Claude Code | Codex |
|---|---|---|
| Project file | `CLAUDE.md` | `AGENTS.md` (with optional `AGENTS.override.md`) |
| User-global file | `~/.claude/CLAUDE.md` | `~/.codex/AGENTS.md` (override variant supported) |
| Discovery | Walk to git root | Walk root → CWD; concatenate top-down with `--- project-doc ---` separator |
| Size cap | None enforced | `project_doc_max_bytes` (default 32 KiB) |
| Fallback names | n/a | `project_doc_fallback_filenames = ["CLAUDE.md", ...]` config key lets `CLAUDE.md` count as Codex's project doc |

**Implication:** A single canonical project doc can serve both. Either emit `AGENTS.md` and symlink/import from `CLAUDE.md`, or list `CLAUDE.md` in `project_doc_fallback_filenames` and stop maintaining `AGENTS.md`.

## 2. Skills

Codex skills are **the same SKILL.md format Anthropic pioneered**, with progressive disclosure. Frontmatter requires `name` + `description`.

**Discovery roots (precedence: REPO → USER → ADMIN → SYSTEM):**

| Scope | Path |
|---|---|
| REPO | `$CWD/.agents/skills`, walked to `$REPO_ROOT/.agents/skills` |
| USER | `$HOME/.agents/skills` (canonical) and `~/.codex/skills/` (deprecated alias) |
| ADMIN | `/etc/codex/skills` |
| SYSTEM | Embedded built-ins |

Note: Codex skills live under `.agents/skills/`, **not** `.codex/skills/`. This is the cross-tool convention — `.agents/skills/` works for any agent honoring agentskills.io.

Optional sidecar `agents/openai.yaml` per skill: `interface { display_name, brand_color, icon_small }`, `policy { allow_implicit_invocation }`, `dependencies { tools }`.

**Invocation:** implicit (description-matched) or explicit (`$skill-name` in composer / `/skills`).

**Discovery budget:** Codex injects skill names+descriptions capped at "roughly 2% of context window, or 8000 chars when unknown."

## 3. MCP servers

Best-in-class. Lives in `~/.codex/config.toml` (TOML, not JSON):

```toml
[mcp_servers.cas]
command = "cas"
args = ["mcp", "serve"]
env_vars = ["CAS_HOME"]

[mcp_servers.cas.env]
CAS_LOG = "info"
```

**Two transports:** `stdio` (command/args/env/cwd) and **streamable HTTP** (url + bearer-token-from-env or static/env headers). Plus full **OAuth flow** (`codex mcp login <name>`, `mcp_oauth_callback_port`).

**Per-server keys:** `startup_timeout_sec`, `tool_timeout_sec`, `enabled`, `required` (fail startup if init fails), `enabled_tools` / `disabled_tools` (allow/deny lists), `default_tools_approval_mode`, `supports_parallel_tool_calls`.

**Per-tool keys** (richer than Claude Code's runtime popup):

```toml
[mcp_servers.cas.tools.task]
approval_mode = "auto"  # auto | prompt | approve
```

**CLI helpers:** `codex mcp add <name> --env K=V -- <command>`, `codex mcp login`, `/mcp` in TUI.

**Plugin-bundled MCP:** `.codex-plugin/plugin.json` references `./.mcp.json` (Claude-compatible format).

## 4. Hooks (the critical correction to current Cassy state)

**Codex has a fully functional hook system** that is ~95% wire-compatible with Claude Code's. The `cas-mux/harness.rs:27` claim that Codex `supports_hooks: false` is outdated.

**Six events** (`codex-rs/hooks/src/lib.rs:11-30`):

| Event | Matcher | Decision verbs |
|---|---|---|
| `SessionStart` | source: `startup`/`resume`/`clear` | additionalContext via `hookSpecificOutput` |
| `UserPromptSubmit` | (none) | `decision: "block"` |
| `PreToolUse` | tool name | `permissionDecision: "deny"` (or exit 2) |
| `PermissionRequest` | tool name | `allow`/`deny` (pre-approval gate) |
| `PostToolUse` | tool name | replace tool result via output |
| `Stop` | (none) | `decision: "block"` continues with reason as new prompt |

**Wire format identical to Claude Code:** JSON on stdin with `session_id`, `transcript_path`, `cwd`, `hook_event_name`, `model`, plus event-specific fields. Stdout JSON uses `hookSpecificOutput.additionalContext`.

**Configured in `~/.codex/config.toml` or `~/.codex/hooks.json`:**

```toml
[features]
codex_hooks = true  # default true now

[[hooks.PreToolUse]]
matcher = "^Bash$"
[[hooks.PreToolUse.hooks]]
type = "command"
command = "/path/to/script"
timeout = 30
status_message = "Checking command"
```

**Three handler types** (`codex-rs/config/src/hook_config.rs:107`): `Command { command, timeout_sec, async, status_message }`, `Prompt {}` (placeholder for future), `Agent {}` (placeholder).

**Plugins ship hooks the same way Claude plugins do** — `.codex-plugin/plugin.json` referencing `./hooks/hooks.json`.

**All matching hooks from all config layers run** — higher precedence does not replace lower; they all fire concurrently.

**Caveats explicitly stated in docs:**
- `PreToolUse` doesn't intercept `unified_exec` background terminals fully, nor `WebSearch` / non-shell-non-MCP tools.
- `PreToolUse` `permissionDecision: "allow"`/`"ask"`, `updatedInput`, `additionalContext`, `continue: false` are "parsed but not supported yet, so they fail open." Use `permissionDecision: "deny"` or exit 2 + stderr to block.
- `HookToolKind` is `Function | Custom | LocalShell | Mcp` — what Claude sends as `Bash` arrives as `LocalShell`. Tool-name pattern matchers need a translation table.

## 5. Subagents

**Codex has TOML-defined subagent roles** at `~/.codex/agents/*.toml` (personal) or `<repo>/.codex/agents/*.toml` (project). Built-ins: `default`, `worker`, `explorer`. The `cas-mux/harness.rs:28` claim `supports_subagents: false` is outdated.

```toml
name = "reviewer"
description = "PR reviewer focused on correctness, security, missing tests."
model = "gpt-5.5"
model_reasoning_effort = "high"
sandbox_mode = "read-only"
developer_instructions = "..."
nickname_candidates = ["Atlas", "Delta"]

[mcp_servers.openaiDeveloperDocs]
url = "https://developers.openai.com/mcp"
```

Roles are loaded as **a real config layer** (full config.toml schema), so a role can override model/provider/profile/MCP servers/skills.

**Critical semantic difference vs Claude:** Codex only spawns subagents on **explicit user request** — there is no autonomous Task-tool delegation analog. Cassy factory orchestration that depends on the assistant choosing to delegate (e.g., cas-code-review fires automatically) needs to either land as a skill the agent invokes, or trigger via Stop/PostToolUse hook.

Global limits: `[agents] max_threads = 6, max_depth = 1, job_max_runtime_seconds`. `/agent` switches threads in TUI.

Experimental `spawn_agents_on_csv` for batch fan-out across rows with `output_schema`, `max_concurrency`.

## 6. Slash commands

**Closed enum.** No user-defined slash commands. Built-ins (`codex-rs/tui/src/slash_command.rs:12-75`): `/permissions`, `/agent`, `/skills`, `/hooks`, `/memories`, `/mcp`, `/plugins`, `/clear`, `/compact`, `/copy`, `/diff`, `/init`, `/model`, `/fast`, `/plan`, `/personality`, `/ps`, `/stop`, `/fork`, `/side`, `/resume`, `/new`, `/quit`, `/review`, `/status`, `/debug-config`, `/statusline`, `/title`, `/keymap`, `/feedback`, `/logout`.

**Substitute is skills (`$skill-name`)**. Codex's migration crate `external-agent-migration` already converts `.claude/commands/*.md` to skills with prefix `source-command-`.

Deprecated path: `~/.codex/prompts/*.md` invoked as `/prompts:<name>` with `$1..$9`/`$ARGUMENTS` placeholders.

## 7. Permissions / sandboxing

Two orthogonal axes:

- **`sandbox_mode`**: `read-only` | `workspace-write` (default) | `danger-full-access`
- **`approval_policy`**: `untrusted` | `on-request` | `never` | `granular { sandbox_approval, rules, mcp_elicitations, request_permissions, skill_approval }`

**OS enforcement:** macOS Seatbelt (`sandbox-exec`); Linux/WSL2 `bwrap` + seccomp; native Windows sandbox.

**Always read-only inside writable roots:** `<root>/.git`, `<root>/.agents`, `<root>/.codex`.

**Workspace-write network is off by default:** `[sandbox_workspace_write] network_access = true` to enable.

**No Claude-Code-style allow/deny pattern list.** Closest equivalents:
- Per-MCP-tool `[mcp_servers.<id>.tools.<tool>] approval_mode = "auto"|"prompt"|"approve"` (covers MCP surface).
- Starlark `prefix_rule` files at `~/.codex/rules/*.rules`, `<repo>/.codex/rules/*.rules` — controls commands run **outside** the sandbox. Test with `codex execpolicy check --rules <file> -- <argv>`.
- Named permission profiles (`default_permissions = ":workspace"`).

**Rules are powerful but verbose** — Starlark with `prefix_rule(pattern=[...], decision="allow"|"prompt"|"forbidden", justification="...", match=[...], not_match=[...])`. Most-restrictive wins.

**`approvals_reviewer = "user" | "auto_review"`** — a guardian sub-agent can approve approvals automatically. Default policy at `codex-rs/core/src/guardian/policy.md`.

## 8. Settings precedence

Highest to lowest (`codex-rs/config/src/loader/mod.rs:80-94`):

1. CLI flags / `--config|-c key=value`
2. Profile values from `--profile <name>` (`[profiles.<name>]` tables)
3. Project config `.codex/config.toml`, root → CWD (closest wins; **trusted projects only**)
4. User config `~/.codex/config.toml`
5. System config `/etc/codex/config.toml`
6. Built-in defaults

Plus enterprise `requirements.toml` (managed config) which can disallow specific values like `approval_policy = "never"`.

`CODEX_HOME` env var overrides `~/.codex` location entirely. `/debug-config` prints live merged layer order.

**No equivalent to `.claude/settings.local.json`** (personal-but-uncommitted tier). Either commit `.codex/config.toml` or gitignore it.

## 9. Plugins & marketplaces

**First-class extension system.** Manifest at `<plugin-root>/.codex-plugin/plugin.json`:

- `skills` → `./skills/` (folder of skills)
- `mcpServers` → `./.mcp.json` (Claude-compatible)
- `hooks` → `./hooks/hooks.json` (auto-loaded if path omitted)
- `apps` → `./.app.json`
- `interface { displayName, brandColor, composerIcon, ... }`

**Codex auto-discovers Claude plugins:** `codex-rs/utils/plugins/src/plugin_namespace.rs:8-9` reads **both** `.codex-plugin/plugin.json` AND `.claude-plugin/plugin.json`. **A Cassy plugin authored for Claude with `.claude-plugin/plugin.json` is recognized by Codex without changes.** This is a significant integration shortcut.

**Marketplaces:** `codex plugin marketplace add owner/repo[@ref]`, supports git-backed (`source: "url"`/`"git-subdir"` with `ref`/`sha`) and local. Reads Claude-style `.claude-plugin/marketplace.json` too. Cache: `~/.codex/plugins/cache/$MARKETPLACE/$PLUGIN/$VERSION/`.

## 10. Headless mode

`codex exec [PROMPT]` — equivalent of `claude -p`.

**Key flags** (`codex-rs/exec/src/cli.rs:9-82`):
- `--sandbox|-s read-only|workspace-write|danger-full-access`
- `--ask-for-approval|-a never|on-request|untrusted`
- `--profile|-p <name>`, `--cd|-C <path>`, `--add-dir <path>`
- `--config|-c key=value` (TOML override; repeatable)
- `--ephemeral` — don't persist session rollout
- **`--json`** — clean JSONL: `thread.started`, `turn.started`, `turn.completed`, `turn.failed`, `item.started`, `item.updated`, `item.completed`, `error`. Items: `agent_message`, `reasoning`, `command_execution`, `file_changes`, `mcp_tool_call`, `web_search`, `plan_update`.
- `--output-last-message|-o <path>`
- `--output-schema <schema.json>` (final response conforms to JSON Schema)
- `--ignore-user-config`, `--ignore-rules`
- `--skip-git-repo-check` (Codex requires git repo by default)
- `CODEX_API_KEY` env var only honored in `codex exec`
- Resume: `codex exec resume --last "prompt"` or `codex exec resume <SESSION_ID>`

**SDK alternatives:**
- TypeScript: `npm install @openai/codex-sdk` → `new Codex().startThread().run("prompt")`
- Python: experimental, JSON-RPC against local Codex app-server

**Remote app-server:** `codex app-server --listen ws://0.0.0.0:4500 --ws-auth capability-token`; clients via `codex --remote ws://host:port`.

## 11. Model providers — the wire-protocol gap

**Pluggable, but `wire_api = "responses"` is the only supported value.** Chat Completions removed.

```toml
[model_providers.proxy]
name = "OpenAI via proxy"
base_url = "http://proxy.example.com"
env_key = "OPENAI_API_KEY"

[model_providers.local_ollama]
name = "Ollama"
base_url = "http://localhost:11434/v1"
```

Built-in IDs: `openai`, `ollama`, `lmstudio`, `amazon-bedrock` (reserved). Auth options: `env_key`, `experimental_bearer_token`, `requires_openai_auth = true`, command-backed `[model_providers.<id>.auth] command = "..."`.

**Anthropic models cannot be pointed at directly** — Codex speaks only the OpenAI Responses API. To use Claude with Codex requires a Responses-API shim (LiteLLM, AWS Bedrock proxy). Note that local Ollama / LM Studio work via `--oss` and `oss_provider = "ollama" | "lmstudio"`.

Recommended models per docs: `gpt-5.5`, `gpt-5.4`, `gpt-5.4-mini`, `gpt-5.3-codex`, `gpt-5.3-codex-spark` (research preview, ChatGPT Pro only).

## 12. Memory subsystem (overlap with Cassy)

Codex has built-in `~/.codex/memories/` thread summarization (`MemoriesToml` config block; `memories/{read,write}` crates). Auto-generates from threads when `[features] memories = true`.

**Recommendation:** Disable Codex memories (`generate_memories = false, use_memories = false`) so Cassy owns memory unambiguously, OR accept dual systems and document which is authoritative.

## 13. Statusline

Closed-set list of built-in items (`codex-rs/config/src/types.rs:631-636`): `model-with-reasoning`, `current-dir`, etc. **No `statusLine.command` shell hook**. Cassy's current Claude-Code statusline shell command has no equivalent. Workaround: Stop/SessionStart hook that emits a notification or sets terminal title.

## 14. Migration crate (the gift)

`codex-rs/external-agent-migration/src/lib.rs` is OpenAI's reference Claude → Codex importer:
- `import_subagents(source_agents, target_agents)` — converts `.claude/agents/*.md` to Codex roles.
- `import_commands` — converts `.claude/commands/*.md` to skills with prefix `source-command-`.
- `count_missing_commands` — drift detection.
- Imports `.claude/hooks/` directory.

Cassy doing the reverse (read Codex artifacts) is the symmetric problem; this crate is the reference for how to do it cleanly.

## 15. Latest release & ecosystem

- **Codex CLI 0.128.0** released 2026-04-30 (alpha 0.129.0-alpha.2 same day).
- ~67K GitHub stars, Apache 2.0, ~95% Rust after the June 2025 TS→Rust rewrite.
- ~696 releases in 12 months.
- 14M monthly npm downloads (March 2026), 3M weekly active users (April 8, 2026, per Sam Altman). Claude Code: 46.3M monthly, larger but slower-growing.
- JetBrains AI Pulse Survey (Jan 2026, 10K devs): 18% Claude Code at work, 18% Cursor, 3% Codex (predates desktop launch).
- Reddit "raw preference" survey (500 devs early 2026): 65% Codex / 35% Claude Code daily; blind code-quality reviews flip to 67% Claude Code. Most pros run both.

**GPT-5.5 ("Spud") released 2026-04-23** — 8 days before this doc. ChatGPT Plus/Pro/Business/Enterprise + Codex with 400K context. API pricing $5/M input, $30/M output. Terminal-Bench 2.0 = 82.7%. Native Computer Use, in-app browser for local dev servers.

## 16. AGENTS.md as a standard

- **Donated to Linux Foundation Agentic AI Foundation December 2025** alongside MCP (Anthropic) and Goose (Block).
- Spec home: [agents.md](https://agents.md/) and [github.com/agentsmd/agents.md](https://github.com/agentsmd/agents.md).
- Originators: Google, OpenAI, Factory, Sourcegraph, Cursor.
- **Read natively by:** Codex CLI, GitHub Copilot (server-side since Aug 2025), Cursor, Windsurf, Amp, Devin, Aider, Jules, Factory, Zed.
- **Claude Code:** still primarily reads `CLAUDE.md`; reads `AGENTS.md` only via symlink. Native support pending.
- **Common pattern:** AGENTS.md as shared cross-tool instructions; `CLAUDE.md` either symlinks or `@./AGENTS.md` imports plus Claude-specific bits.

## 17. Cross-tool ecosystem (Cassy-spirit projects)

None own Cassy's full plane (memory + tasks + rules + skills + search + factory orchestration), but several reach for cross-harness orchestration:

- **Archon** (April 2026) — "first open-source harness builder for orchestrating Claude Code and Codex." YAML-defined deterministic workflows with `agent:` declarations + adapters.
- **Citadel** — Parallel agents in isolated worktrees, four-tier intent routing, persistent campaign memory across sessions. Closest Cassy analog.
- **OpenClaw** — Plugin offering unified `ISession` interface for Claude/Codex/Gemini/Cursor with multi-agent council.
- **everything-claude-code** ("Anthropic Hackathon Winner") — Cross-tool agents/skills/hooks/rules/MCP for Claude Code, Codex, Cursor, OpenCode, Gemini.
- **CodexMonitor** (MIT, Tauri) — Multi-agent orchestrator using Codex app-server protocol.
- **NVIDIA OpenShell** (GTC 2026) — Policy-driven sandbox runtime (Landlock + seccomp + OPA/Rego HTTP CONNECT proxy) for Claude Code, Codex, Cursor, OpenCode.

**Driver/worker pattern** ([Fountain City blog](https://fountaincity.tech/resources/blog/codex-claude-code-harness-together/)): Claude Code drives, spawns Codex via `codex mcp-server`, re-plans on Codex output. Capability gains stack rather than average.

**Cross-tool skill libraries:**
- VoltAgent's `awesome-agent-skills` (1000+).
- `alirezarezvani/claude-skills` (232+, native plugins for Claude/Codex/Gemini).
- `agent-skill-creator` ("one SKILL.md, 14 tools" with auto-detect installer).

**Convergent install pattern:** `~/.claude/skills/` + `~/.agents/skills/` with symlinks for both-tool discovery.

## 18. Top user complaints about Codex (gaps Cassy could fill)

Synthesized from Reddit/HN/WhatsTechIn:

1. **Rate limits / quota chaos** — arbitrary weekly resets; single prompts can consume 7% of weekly limit.
2. **Long-session drift** — erratic behavior past a few hours, especially frontend.
3. **Slowness** — significantly slower per-task than Claude Code; bad for pair-programming flow.
4. **Unreviewable Python-based edits** — Codex often uses Python to mutate files instead of structured edit tools, producing diffs that are hard to review mid-flight.
5. **Thin harness** — "just a really good model wrapped in an okay harness." Lacks Claude Code's hook depth, agent teams, checkpoint/rewind, statusline customization.

Items 4 and 5 are exactly the gaps a Cassy+Codex integration could fill — Cassy's task tracking provides a checkpoint surface; Cassy's skills/rules/memory provide the harness depth Codex lacks natively.

---

## Reference file map (in `/tmp/codex-research/codex-rs/`)

| File | What's in it |
|---|---|
| `config/src/config_toml.rs` | Full TOML config schema (`ConfigToml` struct, lines 91-450) |
| `config/src/mcp_types.rs` | `McpServerConfig`, transports, per-tool approval modes |
| `config/src/hook_config.rs` | Hook config structures, `HookHandlerConfig` enum |
| `config/src/skills_config.rs` | `SkillsConfig`, per-skill enable/disable |
| `hooks/src/types.rs` | Hook payload wire format (`HookPayload`, `HookEventAfterToolUse`, etc.) |
| `hooks/src/lib.rs` | Event name constants |
| `core-skills/src/loader.rs` | Skill discovery roots, frontmatter rules |
| `core-plugins/src/manifest.rs` | Plugin manifest format |
| `utils/plugins/src/plugin_namespace.rs` | Dual `.codex-plugin`/`.claude-plugin` discovery |
| `external-agent-migration/src/lib.rs` | Reference Claude → Codex importer |
| `external-agent-sessions/src/` | Reference Claude session reader (symmetric to what Cassy wants) |
| `exec/src/cli.rs`, `exec/src/exec_events.rs` | Headless mode, JSONL event schema |
| `model-provider-info/src/lib.rs` | Provider abstraction (Responses-API only) |
| `protocol/src/protocol.rs:2922-2927` | `RolloutLine` (on-disk session line) |
| `protocol/src/protocol.rs:2713-2741` | `SessionMeta` (header line) |
| `rollout/src/recorder.rs` | Session writer & file lifecycle |
| `rollout/src/list.rs` | Session enumeration & filename parsing |
| `rollout/src/policy.rs` | Which event types persist to disk |

---

## Stale assumptions in current Cassy code

These will cause silent breakage if not corrected before Codex variant work proceeds:

1. **`crates/cas-mux/src/harness.rs:26-31`** — `SupervisorCli::Codex` declares `supports_hooks: false, supports_subagents: false, supports_textbox_submit: false`. Current Codex (0.128) supports all three. Capability matrix needs review against the empirical data above before any code reads from it.

2. **Tool prefix swap is incomplete** (per 2026-04-28 ideation): `mcp__cas__*` ↔ `mcp__cs__*` per harness — that prefix swap conflicts with Codex's MCP namespacing convention. Codex's `enabled_tools`/`disabled_tools` are richer; a single canonical prefix would be cleaner.

3. **No Codex hook handlers exist** in `cas-cli/src/hooks/handlers/handlers_events/`. The wire format is identical to Claude's, but `HookToolKind` differs (`LocalShell` vs `Bash`). Translation table needed if handlers stay Claude-shaped.

4. **No Codex session reader.** `cas-cli/src/mcp/tools/service/factory_ops.rs:1059-1187` globs `~/.claude/projects/*/<session-id>.jsonl` — hardcoded. Codex sessions live under `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` with a different format. See companion doc `2026-05-01-codex-session-format-spec.md`.

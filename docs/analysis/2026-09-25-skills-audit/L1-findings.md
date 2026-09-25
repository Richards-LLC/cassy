# L1 findings: skill-format standards, cas-writing-for-agents, and Codex/Grok variant parity

Task cas-63c5 · EPIC cas-1660 · 2026-09-25 · author quick-kestrel-65 (Claude Opus 5.5) · repo HEAD `4836e56f7` (v3.31.0) · findings only, no repo file edited.

Tools on this box: `cas 3.31.0 (4836e56)`, `claude 2.1.282`, `codex-cli 0.156.0`, `grok 1.0.41`, `opencode 1.18.23`. Rubric: `L1-rubric.md` (same directory).

## Verdict

The source twins are clean. After prefix substitution, Codex and Grok differ from the Claude canonical only by sanctioned text: two lines in `cas-supervisor.md`, plus the Codex-only checklist twin and `factory-supervisor.md`. The drift guard now covers non-markdown files. The 2026-09-02 findings in this lane's scope are fixed.

The defects are in what each harness actually loads, after `cas update`. Four problems stand out:

- **Bundled scripts never update.** Scripts and examples inside skills stay at the version first installed.
- **Grok and OpenCode load the Claude flavour.** In a checkout that has `.claude/skills`, Grok resolves the Claude flavour of every CAS skill, and OpenCode always does.
- **Codex ignores CAS's agent files.** Codex custom agents are TOML files, so every `.codex/agents/*.md` CAS installs is ignored by Codex.
- **Retired agents are never pruned.** Retired managed agents stay in the Agent menu of every Claude session.

The house standard is accurate on structure but out of date on formats and wording:

- It mis-describes `disallowed-tools`.
- It makes a non-portable top-level key (`managed_by`) mandatory.
- It has no per-harness frontmatter matrix and no listing-budget numbers.
- It has none of the 2026 model-era wording rules: drop emphasis, drop verification scaffolding, and avoid conflicting or ask-first language, which stalls GPT-6.

## Ranked findings

Δ tokens: estimated at bytes ÷ 4. "always" means every session or turn, "invoke" means per skill load, "0" means a correctness-only change.

| # | Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|---|
| 1 | P0 | on-demand scripts | `cas-cli/src/builtins.rs:2318-2339` (`sync_builtin_detailed`), `:2534-2549` (`is_reference_owned_by_managed_skill`), `scripts/gen-builtin-reference-history.sh` (`git ls-files '*references*'`) | Skill files that are neither `SKILL.md` nor under `references/` have no frontmatter, so `is_managed_by_cas` is false on both sides. After first install every update is `SkippedNotManaged` and the file is frozen. There are 8 such files: `cas-wizard/template.sh`, `cas-image-generate/scripts/generate-image.sh`, `cas-technical-drawing/scripts/draft.mjs` and `examples/shelf-box.json`, `cas-release-report/scripts/render.py`, `cas-dataviz/scripts/validate_palette.js` and `examples/*`. The SKILL.md bodies that describe these scripts do update, so text and script drift apart. | On this box, with binary 3.31.0 = HEAD, every `SKILL.md` is current. The installed scripts each match exactly one historical revision: `template.sh` = `c4e055525` (2026-08-20; newest `1ed1cacc9`), `generate-image.sh` = `39b6e7dbc` (08-29; newest `9de233fb9` 09-23), `draft.mjs` = `6d709e1a1` (09-06; newest 09-23), `render.py` = `a1e5d0db3` (09-08; newest `985a2e0d7`). The same holds in `~/.claude-daniel@…/skills`, `~/.codex/skills` and `~/.grok/skills`. | Treat every child of a managed skill directory as owned: extend `is_reference_owned_by_managed_skill` beyond `references/`, and widen the ledger glob to `cas-cli/src/builtins/**/skills/*/**` minus `SKILL.md`. Also add an install-parity doctor check that compares hashes against the embedded catalog. | 0 |
| 2 | P1 | always (skill listing + bodies) | `.claude/skills/*` in any checkout; `builtins.rs:2792-2821` (project sync writes only the harness in use) | Grok walks `.claude/skills` at "High" priority, which beats `~/.grok/skills` ("Lowest"). In the cas-src main checkout, where the supervisor runs, Grok therefore resolves all 42 CAS skills from `.claude/skills` in the `mcp__cas__` spelling instead of Grok's `cas__`. Grok also loads `~/.claude/skills/cas` (Claude-flavoured) everywhere. Factory worktrees have no project skills, so workers there get `~/.grok/skills`, which is correct. | `grok inspect --json` in `/home/pippenz/Petrastella/cas-src`: 42 skills from `…/cas-src/.claude`, and `cas-search`, `cas-worker` and `cas-supervisor` all have source `project` from `.claude/skills`. `~/.grok/docs/user-guide/08-skills.md` shows the priority table and says "Grok scans the Claude and Cursor skill directories by default". The same run in the worktree picks `~/.grok/skills`. | Choose one: (a) write `.grok/skills` on project sync whenever Grok is an installed harness, since the local `.grok` tier outranks `.claude`; (b) spawn Grok with `GROK_CLAUDE_SKILLS_ENABLED=false`; or (c) make skill text prefix-neutral (for example "the CAS `task` tool"), which retires the three spellings entirely (see #13). | 0 |
| 3 | P1 | always | `builtins.rs:1831-1860` (`project_opencode_catalog`), `:2819` (`OpenCode => Ok(SyncResult::default())`) | OpenCode parity is enforced only in tests. The `cas_` projection is never written to disk, so OpenCode loads 41 CAS skills from `.claude/skills` and 1 from `~/.claude/skills`, all in the `mcp__cas__` spelling. OpenCode's MCP tools are named `cas_<tool>`. | `opencode debug skill` in cas-src: 41 of the 42 project skill directories load, and `cas-task-tracking` has location `…/cas-src/.claude/skills/…` with `mcp__cas__` in its body. The one "missing" skill (`cas-supervisor`) comes from `~/.claude/skills`. `opencode_builtin_skills()` has no non-test caller (`grep`: `builtins.rs:2177,2457,8199,8215` only). | Either sync the projection to `.opencode/skills/` (OpenCode's first-party directory, which it scans alongside `.claude/skills`), or adopt option (c) from #2. Until then, stop claiming OpenCode parity in `REQUIRED_FACTORY_CAPABILITIES` docs. | 0 |
| 4 | P1 | agent catalog | `cas-cli/src/builtins/codex/agents/*.md`, `builtins.rs:64-89`, `:2811` | Codex custom agents are **TOML** files that require `name`, `description` and `developer_instructions`, so Codex ignores `.codex/agents/*.md`. The five shared agents reach Codex only because `stop_flow.rs:625-650` `include_str!`s them into prompts; the installed copies do nothing. `codex/agents/factory-supervisor.md` has **no consumer at all**: no Rust reads it and Codex does not load it. It still carries a generated spawn-recipe block that someone has to maintain. | See https://developers.openai.com/codex/subagents (TOML, `developer_instructions` required). The `codex-cli 0.156.0` binary strings contain "must define `developer_instructions`" and "developer_instructions cannot be blank". `grep -rn factory-supervisor cas-cli/src crates --include=*.rs` finds only a doc comment and unrelated test labels. | Either emit Codex agents as TOML (`developer_instructions` = body) so Codex supervisors can delegate natively, or stop installing agent files for Codex and keep the `include_str!` path. Delete `factory-supervisor.md` or move its constraints into `cas-codex-supervisor-checklist`. | −0 runtime; −44 maintained lines |
| 5 | P1 | always (Claude Agent tool description) | `builtins.rs:3192-3237` (skill prune only); no agent prune exists | Retired managed agents are never removed from installs. `code-reviewer` (marked "DEPRECATED — replaced by the cas-code-review skill", which is itself retired), `git-history-analyzer` and `issue-intelligence-analyst` are still installed in `~/.claude-daniel@…/agents`, `~/.codex/agents` and `~/.grok/agents`. They appear in every Claude session's Agent tool list; this session's list includes all three. Only half of the 2026-09-02 retirement took effect: the catalogs are clean (`test_retired_agents_stay_out_of_every_catalog`), the installs are not. | `ls` of the three directories; `head` shows `managed_by: cas`; the retired agents are 157 and 153 lines. | Add `prune_stale_cas_agent_files` that mirrors the skill prune (require `managed_by: cas` and absence from the harness catalog). Run it from `cas update --user` and project sync. | −~120 always (3 descriptions), and removes a misroute risk |
| 6 | P1 | invoke / house standard | `skills/cas-writing-for-agents/SKILL.md:30`; `skills/cas-worker.md:5-7` | The standard says `disallowed-tools` "removes tools from the skill's session". In Claude Code the tools are removed only "while this file is active… **Cleared when the user sends the next message**". Grok, Codex and OpenCode ignore the key. So cas-worker's `disallowed-tools: [TodoWrite, EnterPlanMode]` lapses at the first supervisor message and never applies outside Claude. | `claude 2.1.282` binary schema string: `"disallowed-tools":…describe("Tools removed from the model while this file is active. … Cleared when the user sends the next message.")`. Grok's field table (`08-skills.md`) has no `disallowed-tools`. OpenCode recognises only `name, description, license, compatibility, metadata` (https://opencode.ai/docs/skills). | Correct the standard: "turn-scoped, Claude-only; not a guard". Move the TodoWrite/EnterPlanMode ban to the PreToolUse hook that already polices workers, or drop it. | −~15 invoke |
| 7 | P1 | always (every project) | `AGENTS.md:1-18` (generated by `cas sync agents-md`, `cli/sync/agents_md.rs`) | AGENTS.md is a cross-harness file: Codex, Grok and OpenCode all read it. CAS generates it in the **Codex** spelling (`mcp__cs__`) with Claude Code's `ToolSearch(query="select:…")` syntax. Grok reads *both* `CLAUDE.md` and `AGENTS.md` in each directory, so Grok gets the directive twice, in two spellings, neither of them `cas__`. OpenCode's first-match-per-category rule means it reads AGENTS.md only, in `mcp__cs__` where its tools are `cas_*`. The block opens with `# IMPORTANT:` and `**DO NOT USE…**`, the emphasis style both vendors now say causes over-triggering. | The quoted lines. Grok loads `AGENTS.md`, `CLAUDE.md`, … per directory (https://docs.x.ai/build/features/project-rules). For OpenCode, "first matching file wins in each category" (https://opencode.ai/docs/rules). `ToolSearch` appears twice in Codex's strings, but the `select:` grammar is Claude's; Codex's `tool_search` argument shape is unverified. | Generate a prefix-neutral AGENTS.md that names tools by bare name ("CAS `task` tool"), drop the ToolSearch line from the AGENTS.md projection, and rewrite the heading as plain imperatives: "Track tasks with the CAS `task` tool, not TodoWrite." | −~60 always per harness (caps and ToolSearch line); −~450 for Grok (duplicate block) |
| 8 | P2 | always (listing in every harness) | `cas-cli-craft/SKILL.md:3` (514 chars), `cas-ui-craft` (428), `cas-technical-drawing` (398), `cas-dataviz` (307), `cas-image-generate` (274) | Five descriptions exceed 250 chars. The 39 model-invocable CAS descriptions total 6,803 chars. When Codex does not know the context window, its whole skill listing gets an 8,000-char budget ("2% of context window, or 8,000 characters"); Codex then shortens descriptions first and may omit skills. CAS alone uses 85% of that fallback. OpenAI's 2026-09-11 Astra guidance says descriptions should be "as short as possible"; over-emphasis causes irrelevant loads. | Per-file char counts (python frontmatter parse). https://developers.openai.com/codex/skills. https://developers.openai.com/blog/rethinking-skills-and-prompts-for-gpt-6-astra | Cap at 250 chars (key use case first, one boundary clause) and pin it in `builtin_skill_description_test.rs` for every skill, not just `OWNED_SKILLS`. | −~170 always (671 excess chars), × harnesses |
| 9 | P2 | frontmatter (portability) | 41/41 `SKILL.md` (`managed_by: cas` top-level); `cas-writing-for-agents/SKILL.md:28` makes it required | Top-level custom key. Claude Code, Grok and Codex ignore it silently, but claude.ai uploads, the Skills API and `package_skill.py` reject any key outside `name, description, license, compatibility, metadata, allowed-tools` ("Unexpected key(s) in SKILL.md frontmatter"). The open-standard validator flags it too. | https://code.claude.com/docs/en/skills (portability rule). https://agentskills.io/specification (six fields) | Move it to `metadata:\n  managed_by: cas`. `is_managed_by_cas` (`builtins.rs:2205-2215`) substring-matches `managed_by: cas` inside the frontmatter, so old and new installs both stay managed. Keep the substring check for one release so installed copies still carrying the old top-level `managed_by: cas` are still recognised as managed. | +~2 invoke |
| 10 | P2 | listing (Codex/OpenCode) | `cas-nuxt-playwright/SKILL.md`, `cas-to-questionnaire/SKILL.md` (`disable-model-invocation: true`) | Only Claude Code and Grok honour `disable-model-invocation`. The Codex parser reads only `name`, `description` and `metadata.short-description`; OpenCode ignores unknown keys. Both skills are therefore model-invocable and listed in Codex and OpenCode. | Codex `core-skills/src/loader.rs@31519549`; https://opencode.ai/docs/skills | For Codex, ship `agents/openai.yaml` with `policy: {allow_implicit_invocation: false}` in the codex twin. For OpenCode, accept the gap, or deny it with `permission.skill` in the generated config. | −~90 always in Codex/OpenCode |
| 11 | P2 | on-demand + prompt | `codex/agents/factory-supervisor.md:12`, `codex/skills/cas-codex-supervisor-checklist.md:62`, `cas-cli/src/ui/factory/app/mod.rs:2551` | These tell Codex supervisors not to use `/cas-start`, `/cas-context` and `/cas-end`, but those commands exist in no harness or tree (`grep -rn cas-start cas-cli/src` finds only these). A dead prohibition costs tokens and invites a search for the commands. | grep | Delete all three lines. | −~25 always (spawn prompt) |
| 12 | P2 | house standard | `cas-writing-for-agents/SKILL.md` (whole) | This is a gap against the 2026 research; see the section-by-section audit below. | — | Rewrite as proposed below. | +~250 invoke (on-demand only) |
| 13 | P2 | maintenance | `builtins.rs:92-1830` (3 × 138 `include_str!`), `builtin_flavor_drift_test.rs` | The three-spelling model (`mcp__cas__`, `mcp__cs__`, `cas__`, plus the in-memory `cas_`) produces 414 embedded skill files and a 1,588-line drift test. Findings #2, #3 and #7 show the per-harness spelling does not reach the harness anyway, because each harness reads the others' directories. | Counts from the registration script (138 = 138 on disk for each catalog). | Evaluate one prefix-neutral catalog that names tools by bare name (`task`, `memory`, `coordination`) and states the per-harness prefix once in the always-loaded role guidance. This is an architecture decision for the EPIC and should not be done piecemeal. | −2 × catalog maintenance |
| 14 | P3 | test comment | `cas-cli/tests/builtin_skill_description_test.rs:41-43` | The comment says "Claude Code truncates a skill description past this length (1024)". Claude Code's listing actually truncates the combined `description` + `when_to_use` at **1,536** chars (raised from 250 in v2.1.105). 1,024 is the spec, API, Codex and OpenCode hard limit. | https://code.claude.com/docs/en/skills; https://github.com/anthropics/claude-code/issues/47627 (2026-04-13) | Change the comment to: "1,024 = Agent Skills spec / API / Codex / OpenCode hard limit". | 0 |
| 15 | P3 | Grok UX | `skills/release-notes` | Grok has a built-in `/release-notes` command, so the skill is only invocable as `/user:release-notes` or `/local:release-notes`. | `grok inspect --json`: `invocableAs: user:release-notes` | Accept, or rename the skill `cas-release-notes` for parity with the `cas-` prefix convention. | 0 |
| 16 | P3 | references | `cas-supervisor/references/planning.md:1-5`, `model-selection.md:1-5`, `cas-worker/references/close-gate.md:1-5` | Reference files carry `name`/`description`/`managed_by` skill frontmatter. Reference sync is ledger-based, so the frontmatter does nothing and costs tokens on every read. None of the harnesses registers them as skills (verified with `grok inspect` and `opencode debug skill`). This was an open 2026-09-02 P2 and is not a regression. | `head -5` | Delete the frontmatter. | −~40 per read |
| 17 | P3 | watch | `builtins.rs:2756` (`sync_all_codex_builtins` → `~/.codex/skills`) | The Codex loader labels `$CODEX_HOME/skills` a "Deprecated user skills location… kept for backward compatibility"; the canonical location is `~/.agents/skills`. Do **not** move there blindly: Grok and OpenCode also scan `~/.agents/skills`, so they would pick up the `mcp__cs__` spelling. | `codex-rs/core-skills/src/loader.rs@31519549`. The 0.156 binary's own skill-creator still installs to `~/.codex/skills`. | Keep for now. Revisit together with #13. | 0 |

## 1. Research: current skill formats and prompting guidance (fetched 2026-09-25)

Every source was fetched live through Exa or read from the locally installed harness. Vendor doc pages carry no date, so they are marked undated. **NEW** means dated after June 2026.

### 1a. Agent Skills open standard and Anthropic guidance

- The spec defines six frontmatter fields: `name` (required; 1–64 chars; `[a-z0-9-]`, no leading, trailing or double hyphen; **must match the directory**), `description` (required; 1–1,024 chars; "what the skill does and when to use it"), `license`, `compatibility` (≤ 500 chars), `metadata` (string→string map), and `allowed-tools` (experimental). The body should stay under 500 lines and under about 5,000 tokens; the metadata tier costs about 100 tokens; references should be one level deep. https://agentskills.io/specification (undated; the standard was published 2025-12-18 per https://www.anthropic.com/engineering/equipping-agents-for-the-real-world-with-agent-skills)
- Anthropic's authoring best practices say to write the description "**in third person**" and give it the shape "<what it does>. Use when <triggers>." The `name` may not contain "anthropic" or "claude". Keep the body under 500 lines; give references over 100 lines a table of contents; avoid time-sensitive text; use fully qualified MCP tool names. https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices (undated). agentskills.io instead recommends imperative "Use this skill when…" and being "pushy". https://agentskills.io/skill-creation/optimizing-descriptions.md (undated). CAS's "Use when …" satisfies both sources as long as the what-it-does clause is present.
- Claude Code (https://code.claude.com/docs/en/skills, undated, references v2.1.280):
  - All fields are optional and unknown keys are silently ignored.
  - Fields: `name`, `description`, `when_to_use`, `argument-hint`, `arguments`, `disable-model-invocation` (which also removes the description from context), `user-invocable` (false hides the skill from `/` only), `allowed-tools` (**pre-approves** tools for the invoking turn and does not restrict), `disallowed-tools`, `model`, `effort`, `context: fork`, `agent`, `background`, `hooks`, `paths` (globs that limit auto-activation), `shell`. `metadata`, `license` and `compatibility` are accepted but not acted on.
  - The listing truncates `description` + `when_to_use` at **1,536 chars**. The whole listing has a budget of **1% of the context window**; on overflow, the least-invoked skills lose their descriptions first. The docs advise putting the key use case first.
  - After compaction each skill is re-attached at its first 5,000 tokens, within a 25,000-token total.
  - Portability: claude.ai, the Skills API and `package_skill.py` hard-reject keys outside the six standard fields.
  - Local binary 2.1.282 confirms the `disallowed-tools` semantics: "Cleared when the user sends the next message".
- Claude Code subagents (https://code.claude.com/docs/en/sub-agents, undated) accept these fields: `name` (no `:` since v2.1.218, **NEW** 2026-07-22), `description`, `tools`, `disallowedTools`, `model` (`sonnet|opus|haiku|fable|<id>|inherit`), `permissionMode`, `maxTurns`, `skills`, `mcpServers`, `hooks`, `memory`, `background`, `omitClaudeMd` (v2.1.271, **NEW** 2026-09-14), `effort`, `isolation`, `color`, `initialPrompt`, `experimental.cacheTtl` (v2.1.248, **NEW**).
- Sizing: aim for under 200 lines per CLAUDE.md. On emphasis: "If you emphasize many lines, none of them stands out." https://code.claude.com/docs/en/memory, https://code.claude.com/docs/en/best-practices (undated)

### 1b. OpenAI Codex

- **Skills:** `name` and `description` are required. Codex discovers `.agents/skills` from the working directory up to the repo root, then `~/.agents/skills`, `/etc/codex/skills` and bundled skills. `agents/openai.yaml` holds `interface`, `policy.allow_implicit_invocation` and `dependencies`. The listing budget is 2% of the context window, or 8,000 chars if the window is unknown; Codex shortens descriptions first. Guidance: "Front-load the key use case and trigger words", and "Explain exactly when this skill should and should not trigger". https://developers.openai.com/codex/skills (undated)
- **Loader source** (`codex-rs/core-skills/src/loader.rs@31519549`): name ≤ 64, description ≤ 1,024. The parser reads only `name`, `description` and `metadata.short-description`. `$CODEX_HOME/skills` is "Deprecated user skills location… kept for backward compatibility"; project `.codex/skills` is still scanned. https://github.com/openai/codex/blob/31519549/codex-rs/core-skills/src/loader.rs
- **NEW** (2026-09-11), "Rethinking skills and prompts for GPT-6 Astra": descriptions "as short as possible". Over-emphasising when to use a skill causes irrelevant loads. Multi-workflow skills should be "a minimal router". "overly specific guidance can now hinder results". https://developers.openai.com/blog/rethinking-skills-and-prompts-for-gpt-6-astra
- **AGENTS.md:** no required fields; the closest file wins. Codex reads `AGENTS.override.md`, else `AGENTS.md`, else fallback names, one file per directory from root to working directory, capped by `project_doc_max_bytes`, which defaults to **32 KiB**. https://agents.md, https://developers.openai.com/codex/guides/agents-md (undated)
- **Custom agents are TOML** (`~/.codex/agents/`, `.codex/agents/`) with `name`, `description` and `developer_instructions` required, plus any config.toml key. The `name` field is authoritative, not the filename. https://developers.openai.com/codex/subagents (undated); confirmed in the codex-cli 0.156.0 binary strings.

### 1c. xAI Grok Build and OpenCode

- **Grok skills** (local `~/.grok/docs/user-guide/08-skills.md`, grok 1.0.41; https://docs.x.ai/build/features/skills-plugins-marketplaces, Exa-dated 2026-08-11 **NEW**):
  - Priority order: `./.grok/skills` (highest), then repo `.grok/skills`, then `~/.grok/skills` (lowest). Grok **also scans `.claude/skills` (High for local/repo), `~/.claude/skills`, `.agents/skills`, `.cursor/skills` by default**, and dedupes by name.
  - Fields: `name`, `description`, `when-to-use` (alias `when_to_use`), `allowed-tools` (does not grant or restrict), `argument-hint`, `user-invocable` (**false hides the skill from the model too**, unlike Claude), `disable-model-invocation` (only the literal `true` counts), `model`, `effort`, `license`, `compatibility`, `metadata`.
  - The body is inlined up to 25,000 tokens.
  - Grok claims full Claude Code compatibility: marketplaces, skills, MCPs, agents, hooks, CLAUDE.md and `.claude/rules`.
- **Grok AGENTS.md:** in each directory Grok reads `AGENTS.md`, `CLAUDE.md`, `CLAUDE.local.md`, `.grok/rules`, `.claude/rules` and more, "loaded in full, with no size cap". https://docs.x.ai/build/features/project-rules (undated)
- **Grok agents:** `.md` files in `.grok/agents/` or `~/.grok/agents/`, with `mcpInheritance` and `tools` frontmatter (local `16-subagents.md`). The full field list is **not verifiable** from official docs.
- **OpenCode skills:** `.opencode/skills/<name>/SKILL.md` (plural), `~/.config/opencode/skills`, and also `.claude/skills`, `~/.claude/skills`, `.agents/skills`. "Only these fields are recognized: name, description, license, compatibility, metadata". The name must match `^[a-z0-9]+(-[a-z0-9]+)*$` and the directory. https://opencode.ai/docs/skills (undated). Local `opencode 1.18.23 debug skill` confirms the Claude directories load.
- **OpenCode agents and rules:** Markdown agents live in `.opencode/agents/` and the filename is the agent name. For rules, "the first matching file wins in each category", so `AGENTS.md` beats `CLAUDE.md`. https://opencode.ai/docs/agents, https://opencode.ai/docs/rules (undated)

### 1d. Prompting guidance for the current model families

- **Anthropic** (general best practices, https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices, undated):
  - Emphasis: "these models may now overtrigger… dial back any aggressive language. Where you might have said 'CRITICAL: You MUST use this tool when...', you can use more normal prompting like 'Use this tool when...'"
  - "If in doubt, use [tool]" causes over-triggering.
  - "Tell Claude what to do instead of what not to do."
  - Examples should be relevant and diverse, and wrapped in tags. Use XML tags for complex prompts. Give the reason behind an instruction.
- **Claude Opus 5** (**NEW**, 2026-07-24), https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-5:
  - Remove explicit verification instructions: "instructions like these cause over-verification". Avoid "double-check your answer".
  - The model delegates to subagents more readily and may widen scope, so constrain scope explicitly.
  - Positive style examples work better than don'ts.
  - "only report high-severity issues" is followed literally, so ask for everything and filter afterwards.
- **Claude Opus 5.5** (**NEW**, 2026-09-22), https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-5-5:
  - Default effort is `medium` and thinking is always on. Lower the effort setting rather than prompting for less thinking.
  - Unattended loops can stop after a text-only turn, so name the early stops to avoid.
  - Wrap pasted text in `<pasted_content>` tags.
- **Claude Fable 5** (https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-fable-5, undated): steer with brief instructions and give the reason. Skills that ask the model to echo its reasoning can trigger the `reasoning_extraction` refusal, so audit skills for show-your-thinking text. A grep of the CAS builtins found 0 such phrases.
- **OpenAI GPT-5.5** (2026-04-23), https://developers.openai.com/api/docs/guides/latest-model?model=gpt-5.5: "Avoid unnecessary absolute rules… `ALWAYS`, `NEVER`, `must`, `only`… for true invariants". Prefer decision rules. Start from the smallest prompt.
- **OpenAI GPT-5.6** (**NEW**, 2026-07-09), https://developers.openai.com/api/docs/guides/prompt-guidance-gpt-5p6: "Favor leaner prompts" (+10–15% on evals, −41–66% tokens). "State each instruction once."
- **OpenAI GPT-6 Sol, Luna and Astra** (**NEW**, 2026-09-22), https://developers.openai.com/api/docs/guides/latest-model:
  - The model is "more sensitive to instructions contained in skills and other files, such as `AGENTS.md`. We strongly recommend auditing skills".
  - "unclear or conflicting guidance in a skill file may cause the model to pause and block work early".
  - Strong "ask first" boundary language may stop work.
  - Drop "run the tests" nudges.
  - The model leans towards lists, tables and Markdown.
- **Where the vendors converge:** remove caps and emphasis except for true invariants; remove verification and "be thorough" scaffolding; keep descriptions short and specific; make the root a short router; say what to do; state each rule once; watch for over-triggering rather than under-triggering.

## 2. Audit of `cas-writing-for-agents` (the house standard)

File: `cas-cli/src/builtins/skills/cas-writing-for-agents/SKILL.md` (59 lines; last changed `aff6d88c5` 2026-09-24). Every 2026-09-02 finding against it has been fixed: it has steps, a Done criterion, frontmatter facts, the three-mirror rule and a line budget, and SKILL-MECHANICS is absorbed. That is pinned by `builtin_doc_hygiene_test.rs:286-345`. What remains is staleness against the research.

| Line | Status | Finding | Proposed text / fix |
|---|---|---|---|
| 3 | OK | "Use when creating or editing a skill, AGENTS.md, CLAUDE.md, or an agent-facing reference document." Trigger-first, 98 chars. | Keep. |
| 18 | Stale, incomplete | "every word spends context on every turn" is right, but the step gives no numbers or format rules. Missing: third-person what-it-does clause plus "Use when"; the ≤ 1,024 hard limit; the ~250-char house target; the budgets (Claude 1% of context with least-invoked dropped first; Codex 2% or 8,000 chars); "key use case first"; the "not for X" boundary when a bundled skill competes; no emphasis words. | Add: "Description: `<what it does>. Use when <trigger>; not for <sibling>.` ≤ 250 chars, key use case first, no emphasis words. Every harness truncates or drops long descriptions from a shared listing budget." |
| 19 | OK | Imperative steps with observable done-states. This matches Codex "imperative steps with explicit inputs and outputs" and Anthropic's degrees-of-freedom advice. | Keep. |
| 20 | OK | One statement per rule, and no restating what the harness enforces. This matches GPT-5.6 "State each instruction once". | Keep. |
| 21 | Partly stale | The reference-file rule is good (under ~15 lines stays inline). It omits: references one level deep, a table of contents above 100 lines, scripts that are *run* rather than read, and `${CLAUDE_SKILL_DIR}` (Claude substitutes it into script paths; seen in the binary). It also omits the P0 from #1: scripts and examples outside `references/` do not update on existing installs. | Add: "Put executable helpers in `scripts/` and tell the agent to run them. Until the sync fix lands, only `SKILL.md` and `references/` update on existing installs." |
| 22 | Wrong model | The "three mirrors, byte-identical apart from prefix" rule assumes each harness reads its own copy. Grok and OpenCode read `.claude/skills` (#2, #3), OpenCode's `cas_` spelling never reaches disk, and AGENTS.md is shared (#7). It also says to regenerate `reference-history.json`, but that ledger covers only `*references*` paths. | Keep the procedure. Add "Grok and OpenCode also load `.claude/skills`; do not rely on the per-harness prefix reaching the model." Revisit if the EPIC adopts #13. |
| 24 | Contradicts the tree | "Done when the file is under ~80 lines". 20 of 41 shipped skills exceed 80 body lines: fallow 386, cas-nuxt-playwright 338, cas-github-issues 240, cas-brainstorm 233, cas-ideate 168, … The external standard is under 500 lines and under ~5,000 tokens. As written, every skill edit fails its own Done criterion. | "Methodology skills ≤ 80 lines; procedural skills ≤ 200; hard ceiling 500 lines / 5 k tokens; beyond that split into references." The `builtin_doc_hygiene_test` pin on "80 lines" still passes. |
| 28 | Non-portable | `managed_by` is made *required* as a top-level key (#9). | "`metadata.managed_by: cas` (portable; the sync check matches either form)." |
| 30 | Wrong | `disallowed-tools` "removes tools from the skill's session". It is actually turn-scoped and Claude-only (#6). The line also gives no per-harness caveat for `disable-model-invocation`: Codex and OpenCode ignore it (#10). | Replace with a harness matrix (next row). |
| — | Missing | **Per-harness frontmatter matrix.** Portable: `name`, `description`, `license`, `compatibility`, `metadata`. Claude and Grok only: `disable-model-invocation`, `argument-hint`, `allowed-tools` (pre-approve, never restrict), `when_to_use` (Grok also accepts `when-to-use`), `paths`, `model`, `effort`. Claude only: `disallowed-tools` (turn-scoped), `context: fork`, `agent`, `hooks`, `shell`, `arguments`. `user-invocable: false` means **different things** in Claude (hidden from user only) and Grok (hidden from model too); avoid it. Codex opt-out lives in `agents/openai.yaml`. | Add as a 6-row table (~25 lines). |
| 55-57 | Aligned, incomplete | "Use positive instructions; a prohibition earns space only for a hard guardrail" matches Anthropic and OpenAI. Missing model-era rules: (a) no CAPS or emphasis words except true invariants, and one at most per file; (b) give the reason with each rule; (c) no verification or "double-check" or "be thorough" scaffolding, because it over-verifies on Opus 5.x; (d) no ask-first or blocking language without a concrete trigger, because it stalls GPT-6; (e) no two rules that can conflict, since GPT-6 blocks on conflicts and GPT-5 burns reasoning on them; (f) examples are copied literally, so give one exact example and label anti-examples; (g) no show-your-reasoning instructions (Fable `reasoning_extraction`). | Add a 7-bullet "Wording for current models" section. |
| 59 | OK | The stale-marker list (phase narration, dated notes, operator facts) is good and matches Anthropic's "avoid time-sensitive information". | Keep. |
| — | Missing | The description promises AGENTS.md and CLAUDE.md guidance, but the body has none. Missing facts: CLAUDE.md under 200 lines; Codex concatenates AGENTS.md root→cwd up to 32 KiB; Grok reads AGENTS.md *and* CLAUDE.md in full with no cap; OpenCode reads AGENTS.md *instead of* CLAUDE.md. So always-loaded blocks must be harness-neutral and deduplicated. | Add a 4-line "Instruction files" section, or narrow the description to skills. |

Net effect of the proposed rewrite: about +25 lines (+~250 tokens), paid only when the skill is invoked. Adopting the rewrite's own ≤ 250-char description cap and the plain-imperative AGENTS.md saves about 230 always-loaded tokens (#7, #8).

## 3. Harness parity

### 3a. Source-level diff, classified

Method: `diff -rq`, then a per-file re-diff after substituting `mcp__cas__` → `mcp__cs__` (Codex) or `cas__` (Grok), for all 138 files × 3 catalogs and 5–6 agents.

| Divergence | Harness | Classification | Note |
|---|---|---|---|
| Tool prefix `mcp__cas__` → `mcp__cs__` / `cas__` in 41+ files | Codex, Grok | **Intended adaptation** | Canonicalised by `normalize_harness_skill_content` (`builtins.rs:2225`) and the drift test's `CANON_TOOL`. |
| `cas-supervisor.md:53` checklist pointer (Claude text names both checklists; Codex names only its own) | Codex | **Intended adaptation** | Sanctioned by the checklist twin. |
| `cas-supervisor.md:55` heading "Heterogeneous Teams (Claude supervisor + Codex workers)" vs "(Grok supervisor + Claude/Codex workers)" | Grok (and Codex) | **Intended adaptation** | `CANON_HETERO`. |
| `skills/cas-supervisor-checklist.md` ↔ `codex/skills/cas-codex-supervisor-checklist.md` (16 changed hunks: no-hooks framing, explicit `cas codemap status`, explicit WIP report) | Codex | **Intended adaptation**, with one stale line | `ALLOWED_FLAVOR_ONLY` and `REQUIRED_FACTORY_CAPABILITIES`. Codex now has CAS-installed PreToolUse/PostToolUse hooks (`~/.codex/hooks.json`, `^Bash$` matcher), so "Session Start (No Hooks)" is still accurate for SessionStart and MCP-tool gates. The `/cas-start` line (#11) is drift, because those commands do not exist. |
| `codex/agents/factory-supervisor.md` (Codex only) | Codex | **Drift by construction** | It is not a Codex agent format and has no consumer (#4). Its 2026-09-02 P0s (untiered spawn, monitoring ban, `/epic-spec`) are **fixed**. |
| Agents (5 shared) | Codex, Grok | Identical after substitution | The Claude-flavour `task-verifier` has `model: inherit`. Whether Grok honours `inherit` in `.grok/agents` frontmatter is **unverified**. |
| Non-markdown files (`template.sh`, scripts) | all | Identical in source | The 2026-09-02 `template.sh` twin drift is **fixed**, and the drift test now walks non-`.md` files (`builtin_flavor_drift_test.rs:371-421`). The install drift is #1. |
| Registration | all | Identical | 138/138/138 skills and 5/6/5 agents registered, equal to what is on disk (python cross-check of `include_str!` against `os.walk`). |

### 3b. Resolved-copy parity (what each harness actually loads)

| Harness | cwd | What loads | Verdict |
|---|---|---|---|
| Claude 2.1.282 | any | `~/.claude-<acct>/skills` (current) plus project `.claude/skills` | Correct flavour. Retired agents are still listed (#5). |
| Codex 0.156 | any | `~/.codex/skills` (deprecated location, still read) plus `.codex/skills`; `.codex/agents/*.md` ignored | Correct skill flavour. Agents are inert (#4). Opt-out skills are listed (#10). |
| Grok 1.0.41 | factory worktree | `~/.grok/skills` wins; `~/.claude/skills/cas` is also loaded (Claude flavour) | Mostly correct. |
| Grok 1.0.41 | cas-src main checkout | All 42 CAS skills come from `.claude/skills` (Claude flavour) | **Wrong flavour** (#2). |
| OpenCode 1.18.23 | cas-src main checkout | 41 from `.claude/skills` and 1 from `~/.claude/skills`; the `cas_` projection is never used | **Wrong flavour** (#3). |
| Codex, Grok, OpenCode | any project | AGENTS.md in the Codex spelling with the Claude ToolSearch line; Grok also loads CLAUDE.md | **Wrong flavour or duplicated** (#7). |

### 3c. How the variants are kept in sync, and where that fails

1. **Authoring:** hand-maintained twins under `builtins/{codex,grok}/`, each registered in `BUILTIN_SKILLS`, `CODEX_BUILTIN_SKILLS` and `GROK_BUILTIN_SKILLS` (`builtins.rs:92,679,1276`). The OpenCode twin is an in-process projection (`:1831`) used only by tests.
2. **Gate:** `cas-cli/tests/builtin_flavor_drift_test.rs` (1,588 lines). It canonicalises tool prefixes, config directories, catalog constants and hetero headings, then compares section by section, including non-markdown files since cas-ef87a. Supporting guards are `builtin_skill_description_test.rs`, `builtin_doc_hygiene_test.rs` and `factory_parity_test.rs`. Failure: all of these compare **source against source**. None compares an installed copy against the catalog, which is how #1 and #5 went unseen.
3. **Install:** `sync_all_builtins_inner` (`:2658`). `SKILL.md` and agents are overwritten when either side is `managed_by: cas`. `references/*` are overwritten when the destination matches a hash in `reference-history.json`; a mismatch is treated as a local edit and preserved. Failures:
   - Every other file is gated on frontmatter it cannot have, so it freezes (#1).
   - The ledger generator only globs `*references*`.
4. **Prune:** `prune_stale_cas_skill_dirs` (`:3192`) and `prune_stale_cas_workflow_files` (`:3277`). Failures:
   - There is no agent prune (#5).
   - The skill prune touches only `cas-*` directories, so a retired builtin without the `cas-` prefix (`codemap`, `fallow`, `design-spec`, `project-overview`, `release-notes`, `session-learn`, `verify-before-claim`, `mecha-cassy`, `mcp-integration`, `cli-routing`) would never be pruned.
5. **Cross-harness discovery:** the mechanism assumes one harness reads one directory. Grok, OpenCode and increasingly Codex (`.agents/skills`) read each other's directories, so the per-harness spelling is not what the model sees (#2, #3, #7, #13).
6. **AGENTS.md:** `cas sync agents-md` projects CLAUDE.md into one spelling (Codex), with no Grok or OpenCode variant (#7).

## 4. Regressions and still-open items from the 2026-09-02 review (this lane's scope only)

- **Fixed and verified:**
  - cas-writing-for-agents P1 #18 and P2 #17.
  - Drift guard covers non-markdown files (P2 #18).
  - `template.sh` twin drift.
  - Codex factory-supervisor P0 #5, #6 and #8.
  - Skill-prune inversion R1.
  - Retired agents out of the catalogs.
- **Half-fixed (regression class):** retired agents were removed from the catalogs but never from installs (#5).
- **Still open (not re-scored, noted for lanes L2–L5):**
  - Reference-file frontmatter (#16).
  - `cas-supervisor-checklist.md:45` "cherry-pick into `develop`".
  - The CLAUDE.md directive's caps (#7).

## 5. Search manifest

| Command | Hits |
|---|---|
| `diff -rq builtins/skills builtins/{codex,grok}/skills` (+ agents) | Codex: 1 only-in pair plus factory-supervisor; Grok: 0 |
| per-file re-diff after prefix substitution (skills) | Codex: 1 file (2 lines); Grok: 1 file (2 lines) |
| per-file re-diff after prefix substitution (agents) | 0 / 0 |
| python `include_str!` vs `os.walk` registration check | 138/138/138 and 5/6/5; 0 unregistered, 0 missing |
| python frontmatter scan (name = dir, regex, description length, keys) | 0 name mismatches; 5 descriptions over 250; key counts: `managed_by` 41, `license` 5, `metadata` 5, `disable-model-invocation` 2, `disallowed-tools` 1 |
| `git log` revision match of installed scripts in `~/.claude-daniel@…/skills` | 4 of 4 frozen at first-install revs |
| `cmp` installed scripts in `~/.codex/skills`, `~/.grok/skills` | 2 of 2 differ (each) |
| `ls ~/.claude-daniel@…/agents ~/.codex/agents ~/.grok/agents` | 3 retired agents × 3 dirs |
| `grok inspect --json` (worktree / main checkout) | `~/.grok` wins / 42 skills from `.claude/skills` |
| `opencode debug skill` (main checkout) | 41 skills from `cas-src/.claude/skills`; `mcp__cas__` in body |
| `strings codex \| grep developer_instructions` | 51 matches in raw grep; the TOML "must define" message is present |
| `grep -a disallowed-tools claude-2.1.282` | 5; describe string "Cleared when the user sends the next message" |
| `grep -rn factory-supervisor cas-cli/src crates --include=*.rs` | 1 doc comment + 3 unrelated test labels |
| `grep -rn cas-start cas-cli/src` | 3, all prohibitions of non-existent commands |
| `grep -rhoE 'CRITICAL\|IMPORTANT\|MUST\|NEVER\|ALWAYS\|DO NOT\|MANDATORY' skills agents` | 13 total in 11 files (already low) |
| `grep -rniE 'show your reasoning\|think step by step\|…' skills agents` | 0 |
| Exa fetches (research sub-agent, 57 tool calls) | pages cited inline; working copies in `/tmp/rs/` |

# P3: pstack packaging, cross-harness distribution and learning loops vs Cassy

Task cas-eb5a, epic cas-9081. Findings only. Sources are read-only checkouts:

- `~/research/pstack/cursor-plugins` (upstream `cursor/plugins`, head `78f46da`)
- `~/research/pstack/pstack-claude` (port `michael-denyer/pstack-claude`, head `9f3a2ca`, v0.9.44, synced to upstream `12d587d` = pstack 0.15.5)

## Verdict

pstack ships to Cursor, Claude Code and Codex from **one skill tree with no per-harness copies**. The
text is written in one harness's tool language and adapted three ways: one lookup file, generated
stubs and frontmatter, and a mechanical upstream sync with a denylist. This validates the D1
one-tree direction.

- **D1 (bare names):** Cassy's plan is stronger than pstack's, because our only harness difference is
  the MCP prefix.
- **Twin files:** worth copying are pstack's "generate the harness-only bits" and "denylist harness
  literals in shared text" mechanics; that is how Cassy's remaining twins go away.
- **Learning loops:** pstack's learning loops are manual, operator-approved skill edits. Cassy's are
  store-backed and automatic. Two pstack mechanics are worth adopting: the Stop-hook cadence gate
  with an incremental transcript index, and reflect's "route to structure before prose" check.
- **Plugins:** marketplace plugins cannot replace `cas`-managed installs, because Cassy needs its MCP
  server, hook config and project init. Plugins could complement them later for skills-only
  consumers.

## (a) How pstack ships to three harnesses from one tree

### Upstream: Cursor (`cursor-plugins/pstack`)

| Piece | Path | What it does |
|---|---|---|
| Plugin manifest | `pstack/.cursor-plugin/plugin.json` | `"skills": "./skills/"`, `"agents": "./agents/"`, name/version/logo/keywords. No hooks. |
| Skills | `pstack/skills/<name>/SKILL.md` (47 dirs, 23 of them `principle-*` leaves) | Written in Cursor tool language (`Task`, `AskQuestion`, `.cursor/rules/`). |
| Per-user settings | `setup-pstack` skill → `~/.cursor/rules/pstack-models.mdc` (always-applied rule) | Model per role plus reasoning budget. The skills read it at run time. |
| Distribution | the `cursor/plugins` monorepo is the marketplace; `schemas/` + `scripts/` validate manifests | Users install from Cursor's plugin UI. |

### Port: Claude Code + Codex (`pstack-claude`)

| Piece | Path | What it does |
|---|---|---|
| Claude marketplace catalog | `.claude-plugin/marketplace.json` | Lists plugin `pstack` with `source: ./plugins/pstack`. Install: `/plugin marketplace add michael-denyer/pstack-claude` then `/plugin install pstack@pstack-claude`. |
| Codex marketplace catalog | `.agents/plugins/marketplace.json` | The same plugin, `source: {local, ./plugins/pstack}`, `policy.products: ["CODEX"]`. Install: `codex plugin marketplace add …` then `codex plugin add pstack@pstack-claude`. |
| Claude plugin manifest | `plugins/pstack/.claude-plugin/plugin.json` | Names only the `agents` (2 plus 10 generated effort agents). Claude discovers `skills/` by convention. |
| Codex plugin manifest | `plugins/pstack/.codex-plugin/plugin.json` | `"skills": "./skills/"`, `"hooks": "./hooks/hooks.json"`, plus an `interface` block (display name, default prompts, brand colour). **The same skills directory as Claude.** |
| Codex slash stubs | `plugins/pstack/.codex-plugin/prompts/<skill>.md` (31) | Generated 3-line files: "Invoke the `how` skill and follow it. Resolve Claude tool names … through `poteto-mode/references/codex-tools.md`." |
| The one mapping file | `plugins/pstack/skills/poteto-mode/references/codex-tools.md` (91 lines) | Tables for tool actions (Read→`shell`, Agent→`spawn_agent`, TodoWrite→`update_plan`, AskUserQuestion→plain text), subagent policy, model names, the session hook, driver skills, and a 5-row **Per-skill notes** table. It states that it is Codex-only, "not a cross-runtime map". |
| Platform notes in skills | `poteto-mode/SKILL.md` "Platform Adaptation" section; one line in 14 other skills: "On Codex, read the platform mapping … before following this skill." | The skill text is never rewritten per harness. The model reads the mapping when needed. |
| Session routing hook | `plugins/pstack/hooks/hooks.json` + `hooks/session-start` (sh) + `hooks/session-start-context.md` | One script for both harnesses. `PLUGIN_ROOT` set means Codex (`~/.codex/pstack-models.md`), otherwise Claude (`~/.claude/pstack-models.md`). `session hook: off` in the sheet disables it. Codex requires the user to trust the hook via `/hooks`. |
| Model defaults | `plugins/pstack/models.json` | Stamped into each skill's `## Models` section by the generator, never hand-written in prose. |
| Other harnesses | `docs/reference.md#shared-skills-installation` | Symlink `plugins/pstack/skills/*` into `~/.agents/skills/` (Prime Agent, opencode, Gemini CLI, skills-only Codex), or run `npx skills add <tree> --skill "*" --agent "*"`. CI installs via that CLI and diffs the result against the sources. |
| Generator | `tools/generate.mjs` | Stamps versions, model sections, Codex prompts (order and descriptions come from the README slash-command table), effort agents and portable reference files. `tests/readme-facts.test.mjs` pins skill counts and the upstream pin. |
| Upstream sync | `tools/sync.mjs` + `tools/upstream.json` + `tools/substitutions.json` | A 3-way merge per file between the derived-old, derived-new and local versions: clean update, fork kept, merged, or conflict reported. Upstream-deleted files are deleted only if unforked. `substitutions.json` holds ordered literal or regex rewrites (Task→Agent, AskQuestion→AskUserQuestion, `.cursor/rules/`→CLAUDE.md imports, model-slug shapes→"default in Models"). It also holds a **denylist of Cursor-isms** that fail the run with file, line and hint. The pin advances only on success. |
| What is deliberately not ported | `tools/upstream.json` `exclude` lists; `docs/reference.md` "Port scope and attribution" | Excluded: `.cursor-plugin/`, automations, docs/assets, `make-bot-ui` (Grok Bot UI), and 11 cursor-team-kit skills (control-cli/ui, loop-on-ci, weekly-review, and so on). The file also says "Cursor-specific automations, sticky-mode metadata … the Cursor UI tutorial are excluded". Per-skill port changes are logged in `CHANGES.md`. The sync boundary is in `CONTRIBUTING.md`: upstream owns content, the port owns translation. |

### Contrast with Cassy

| Concern | pstack | Cassy today (epic tip) |
|---|---|---|
| Shared text | one tree in Claude tool language | Three catalogs (`builtins/skills`, `builtins/codex/skills`, `builtins/grok/skills`). WP12a (cas-a638) collapses them to bare names plus one naming line; its progress note reports 277 twin files deleted, with codex/grok catalogs `include_str!`-ing the canonical files. |
| Harness differences | one lookup file plus a one-line pointer where it matters | Prefix remap at load and in role guidance. Twins are kept for codex/grok `cas-supervisor.md`, the codex checklist, 2 `openai.yaml`, task-verifier ×3 (`tools:` frontmatter), and codex `factory-supervisor.md` (D6 deletes it, WP12b). |
| Harness-only artifacts | generated (`generate.mjs`), with tests pinning facts | Hand-maintained twins; the reference ledger is regenerated by `scripts/gen-builtin-reference-history.sh`. |
| Literal guard | the denylist fails the sync | The drift test compares twins after prefix normalization, and the call-shape lint (`cas-cli/tests/mcp_action_surface_test.rs`) checks call validity. Nothing denies a prefixed literal in shared text. |
| Install | the marketplace plugin, or symlinks into `~/.agents/skills` | `cas init`/`cas update`: `sync_builtin_detailed` and `sync_all_builtins_for_project` (`cas-cli/src/builtins.rs:2294,2914`), `prune_removed_owned_skill_files` (`:2807`), and the `reference-history.json` ledger (WP4). Plus MCP registration, hooks, the SessionStart bundle and the AGENTS.md managed block. |
| Hook harness detection | env `PLUGIN_ROOT` in one script | `harness_policy::own_tool_prefix()` from `CAS_*` env (`cas-cli/src/harness_policy.rs:358`). Equivalent. |

**What this means for D1 (sent early as `~/.cas/artifacts/cas-eb5a/d1-note.md`, message 35375):**

1. One tree, no twins, works in production for three harnesses. Bare names plus one naming line is
   the cleaner version of pstack's approach, because pstack still asks the model to translate
   `Agent`→`spawn_agent`, while Cassy's only harness difference is the prefix.
2. pstack keeps a harness-specific difference out of shared text in two ways: a one-line pointer to
   a per-harness reference file, or generation of the harness-only file (frontmatter, stubs). It
   never keeps a full twin. Cassy's kept twins should go the same way.
3. The denylist is what keeps one tree clean over time; a literal slips back in otherwise. Cassy
   needs the equivalent: no `mcp__cas__`/`mcp__cs__`/`cas__<tool>` in shared text outside the
   naming line.

## (b) Learning loops: pstack vs Cassy

| Loop | Trigger | Input | Output | Approval | Cassy counterpart |
|---|---|---|---|---|---|
| pstack `reflect` (`skills/reflect/SKILL.md`, 73 lines + 4 reference prompts) | the user says "reflect" | The active transcript. The parent locates its own JSONL and never globs other workspaces. | Three reviewers in parallel (judgment, tooling, divergent), with a model per role from the models sheet. A synthesizer returns Accepted / Rejected / Backlog, then a **structural-enforcement check** moves anything a lint, script or runtime check could enforce into Backlog. | The user approves the Accepted subset. Trivial edits are applied directly; substantive ones go through the harness's create-skill loop. | `session-learn` skill (`builtins/skills/session-learn/`, opt-in Stop classifier in `hooks/handlers/handlers_session.rs`) produces drafts that go to `cas-memory-management`. The `learning-reviewer` job (`builtins/jobs/learning-reviewer.md`) promotes learnings to rules or skills. Cassy has no three-lens review and no explicit "structure before prose" gate. |
| pstack `recall` (`skills/recall/SKILL.md`, 35 lines) | "catch me up", before resuming | Own transcripts (a 7-day default window, fan-out over cheap subagents) plus the shared record via `why` investigators, verified against live git/gh | A brief with a fixed contract: capsule (≤5), thread lines each carrying exactly one status tag (`[merged #N]`, `[open PR #N]`, `[in flight <branch>]`, `[verified, uncommitted]`, `[reverted #N]`, `[planned, not started]`), problems (≤5), and one next move | none (read-only) | `search action=context`, ambient recall packets, and the `memory entry_type=handoff` injected at session start. Cassy's store is richer (tasks, notes, verified commits), but it has no fixed brief contract and no status-tag vocabulary. |
| pstack `automate-me` (104 lines) | "automate me" | Transcript mining in 3 slices (a signal must appear in 2+ slices) plus structured questions | One personal `<handle>-mode` skill, edited in place on re-runs (`git log -1` sets the mining window) | the user drives it | Operator preferences live as `memory entry_type=preference` and in the user-level `MEMORY.md` files. There is no single generated "mode" document. |
| Cursor `continual-learning` plugin (`continual-learning/hooks/continual-learning-stop.ts`, `agents/agents-memory-updater.md`) | A Stop hook, gated at ≥10 counted turns **and** ≥120 min since the last run (trial mode: 3 turns / 15 min for 24 h), counting only `status == completed && loop_count == 0` turns | Only transcripts new since, or with a newer mtime than, `.cursor/hooks/state/continual-learning-index.json`, which is pruned when transcripts are deleted | A `followup_message` makes the agent run the updater subagent. It edits `AGENTS.md` under two fixed sections (Learned User Preferences / Learned Workspace Facts), ≤12 bullets each, in place, deduplicated. It returns exactly "No high-signal memory updates." when there is nothing. | none (automatic) | The Stop-hook jobs (session-summarizer, duplicate-detector, learning-reviewer, rule-reviewer; `cas-cli/src/maintenance_jobs.rs`). Session-learn runs per Stop when enabled, with **no turns/minutes gate and no incremental transcript index**. Cassy writes learnings to the memory and knowledge stores, not to instruction files; AGENTS.md is a canonical managed block (D3). |

What Cassy has that pstack does not: typed stores (memory tiers, opinions, knowledge pages,
rules with draft/proven promotion), overlap detection, team promotion, and retrieval metrics. pstack
has no memory store at all; its "memory" is the transcript history plus edited skill and AGENTS.md
text.

## (c) Plugin/marketplace distribution vs `cas`-managed installs

- **What a plugin carries:** skills, agents, hooks, slash stubs and (Codex) an interface card, all
  installed and updated by the harness. **What it cannot carry for Cassy:** the `cas` binary and
  `cas serve` MCP registration (project `.mcp.json`, Codex `mcp_servers.cs`), the per-project
  AGENTS.md/CLAUDE.md managed block, the store, and the factory runtime. A Cassy plugin would still
  need `cas init`.
- **Conflict risk:** a plugin install and `cas update`'s project copies of the same skills would give
  the model two copies of each skill, and plugin skills are namespaced (`pstack:…`). The WP4
  prune/ledger logic only owns `.claude/skills`/`.codex/skills`/`.grok/skills`.
- **Where plugins would help:** skills-only consumers who do not run `cas` (for example a teammate
  who only wants `cas-ui-craft`), and auto-update without `cas update`. pstack's
  `npx skills add … --agent "*"` path does the same for harnesses without a marketplace.
- **Recommendation:** skip it as a replacement. Keep it as a later, optional complement, gated on
  the D1 one-tree catalog landing, because a plugin needs exactly that single tree.

## Recommendations

Effort: S ≤ 2 h, M ≈ half a day, L > 1 day.

| # | Adopt / Skip | What | File | Effort |
|---|---|---|---|---|
| 1 | **Adopt (WP12a)** | Denylist test: shared builtin text contains no `mcp__cas__`, `mcp__cs__` or `cas__<tool>` literal outside the one naming line (pstack `substitutions.json` denylist). | `cas-cli/tests/builtin_flavor_drift_test.rs` | S |
| 2 | **Adopt (WP12a/b)** | Replace the frontmatter-only twins (task-verifier ×3 `tools:`) with one body plus frontmatter stamped per harness at install time (pstack `deriveSkill` stamps). | `cas-cli/src/builtins.rs` (`sync_builtin_detailed`, catalog consts) | M |
| 3 | **Adopt (WP12b)** | For genuine per-harness content (codex/grok `cas-supervisor.md`, the codex checklist), use one canonical body plus a per-harness `references/<harness>.md` and a one-line "On <harness>, read …" pointer, not full twins (pstack `codex-tools.md` + Platform Adaptation). | `cas-cli/src/builtins/skills/cas-supervisor.md`, `cas-supervisor-checklist.md` | M |
| 4 | **Adopt** | Cadence gate for the session-learn Stop run (≥N counted turns and ≥M minutes, counted turns only) plus an incremental transcript index keyed by path and mtime, so each Stop mines only the delta (continual-learning stop hook). | `cas-cli/src/hooks/handlers/handlers_session.rs` (session-learn Stop path) | M |
| 5 | **Adopt** | Add reflect's structural-enforcement step to the learning reviewer: before promoting a learning to prose, ask whether a lint, test, hook or schema could enforce it, and route it there instead. | `cas-cli/src/builtins/jobs/learning-reviewer.md` | S |
| 6 | **Adopt** | Give the session handoff and "catch me up" answers recall's fixed contract: capsule ≤5, one status tag per thread, problems ≤5, one next move. | `cas-cli/src/builtins/skills/cas-memory-management/references/body-templates.md` | S |
| 7 | **Adopt (when refreshing fallow)** | Refresh the vendored fallow references with pstack's sync shape: pinned SHA, exclude list, 3-way merge, denylist (audit M54: vendored refs are stale). | `scripts/` (new sync script) + `cas-cli/src/builtins/skills/fallow/` | M |
| 8 | Skip | Continual-learning's AGENTS.md bullet writing. Cassy's AGENTS.md is a canonical managed block (D3), and learnings belong in memory/knowledge, where overlap detection and retrieval apply. | n/a (`cas-core/src/sync/agents_md.rs` stays managed-only) | — |
| 9 | Skip | automate-me's personal `-mode` skill. Preferences already live as typed memory entries surfaced by recall; a generated mode skill would duplicate them and drift. | n/a | — |
| 10 | Skip (defer) | A marketplace plugin as the install path. It cannot register `cas serve`, hooks or the managed block; revisit as an optional skills-only complement once #1–#3 land. | future `plugins/cassy/.claude-plugin/plugin.json`, `.codex-plugin/plugin.json` | L |
| 11 | Skip | Generated Codex slash stubs. Cassy skills are invoked by skill name or through the factory; Codex prompts add a second surface to keep in sync. | n/a | — |

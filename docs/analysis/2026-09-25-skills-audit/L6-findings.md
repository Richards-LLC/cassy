# L6 audit — installed instruction files and non-builtin skills (cas-ea56)

2026-09-25 · warm-marten-55 · EPIC cas-1660 · scored against the L1 rubric v1.1
(`~/.cas/artifacts/cas-63c5/rubric.md`) · baseline `docs/analysis/2026-09-02-builtin-skills-review.md`
· tree `factory/warm-marten-55` (v3.31.0 + audit docs). Findings only; no cargo; nothing run that
writes (`cas sync agents-md --check` and `grok inspect --json` are read-only).

## Verdict

The managed CLAUDE.md block is short, and the ToolSearch line in it is accurate for Claude Code.
Around it there are five problems:

1. **The instructions reach each harness wrong.**
   - Grok loads both CLAUDE.md and AGENTS.md, with two different wrong tool prefixes, and
     truncates CLAUDE.md at 10,000 characters.
   - Codex and Grok get a Claude-only bootstrap (`ToolSearch`, `TodoWrite`).
   - Codex outside cas-src gets no Cassy directive at all.
2. **Copies drift and nothing catches it.**
   - Six installed blocks are stale.
   - 38 projects carry a duplicate of the home-level block.
   - The committed AGENTS.md has been stale since 2026-09-18.
   - Retired agents stay installed in every harness home.
3. **Wrong facts are loaded on every turn.**
   - The init-written `cas` skill documents a task parameter that the schema rejects.
   - A Gabber Studio branching rule is a proven, unconditional rule in cas-src.
   - "Codex does not support hooks" is false.
   - The Ink-crash section cites a task closed five months ago.
4. **The operational text in this repo's CLAUDE.md costs ~1 k tokens on every turn.** About
   half of it is build, CI and sccache policy that only a supervisor doing assembly needs.
5. **None of the 09-02 review's four CLAUDE.md-block findings has been fixed.** The block grew
   by one line (bug routing) instead.

## Always-loaded cost (measured)

Tokens ≈ injected bytes ÷ 4. Claude Code strips block-level HTML comments before injecting
(code.claude.com/docs/en/memory), so "injected" is the raw size minus comment blocks for Claude
rows. Grok and Codex rows are raw.

| File | Written by | Loaded by | Raw B | Injected B | ≈ tok/turn | Lines |
|---|---|---|---:|---:|---:|---:|
| `~/CLAUDE.md` (CAS block + user release-notes section) | `cas init` at $HOME + operator | Claude, every session under ~ | 2,313 | 2,221 | 555 | 30 |
| `~/Petrastella/CLAUDE.md` (stale block only) | old `cas init` | Claude, every session under ~/Petrastella (27+ projects) | 1,083 | 991 | 247 | 17 |
| `cas-src/CLAUDE.md` (block + repo guide) | block: CAS; rest: repo | Claude; Grok (truncated, F3) | 10,410 | 9,878 | 2,469 | 149 |
| `cas-src/AGENTS.md` (generated, stale) | `cas sync agents-md` | Codex; Grok | 8,793 | 8,793 | 2,198 | 129 |
| `cas-src/.claude/rules/cas/rule-002.md` | rule sync | Claude in the main checkout (no `paths:` = unconditional) | 365 | 365 | 91 | 4 |
| `~/.codex/AGENTS.md` = `~/.grok/GROK.md` (byte-identical exa guidance) | operator | every Codex / Grok session | 1,237 | 1,237 | 309 | 26 |
| user auto-memory `MEMORY.md` (not CAS; context only) | Claude auto-memory | Claude in cas-src | 3,821 | 3,821 | 955 | 28 |
| managed block alone | `docs_and_skill.rs:8-22` | — | 1,289 | 1,198 | 300 | 18 |
| `.claude/agents/macos-onboarding-reviewer.md` | repo | description always (373 chars); body on spawn | 7,933 | — | 93 (desc) | 120 |
| skill descriptions: exa-search 520 ch, accounting-client-report 367 ch, `cas` 176 ch | operator / `cas init` | every Claude/Grok session (exa: Codex too) | — | — | 266 | — |
| installed retired agents' descriptions: code-reviewer, git-history-analyzer, issue-intelligence-analyst | stale installs | every Claude/Codex/Grok session | — | — | ≈170 | — |

Per-session totals for the instruction files (excluding SessionStart/spawn guidance, which is
L2/L3):

- **Claude in a cas-src worktree:** ~/CLAUDE.md + ~/Petrastella/CLAUDE.md + repo CLAUDE.md
  ≈ 13.1 KB ≈ **3.3 k tokens**. The managed block appears **three times** (≈ 600 tokens
  redundant). This session's own context shows the same three files.
- **Grok in cas-src:** `grok inspect --json` lists `projectInstructions` = CLAUDE.md (10,410 B,
  2,602 tok) + AGENTS.md (8,793 B, 2,198 tok), plus GROK.md. That is ≈ **5.1 k tokens**, of which
  ≈ 2.1 k repeat the same text.
- **Codex in cas-src:** AGENTS.md + ~/.codex/AGENTS.md ≈ **2.5 k tokens**.
- **Claude in any ~/Petrastella/<project>:** the block ×3 (home, Petrastella, project) ≈ 900
  tokens, 600 of them redundant.

## Where the preamble comes from, and how installed copies compare

- **Template:** `cas-cli/src/cli/init/docs_and_skill.rs:8-22` (`CAS_DIRECTIVE_CONTENT`), wrapped
  by `build_cas_section()` `:25-27`. It is written by `update_claude_md()` `:88-155`, called from
  `cli/init.rs:513,982` and `cli/update.rs:1891`. Since cbb119296 (2026-05-18), injection is
  skipped when any ancestor up to $HOME already has the block (`:37-80,91-93`), but an existing
  descendant block is "left untouched (not deleted)" (`:92`) and is not refreshed either.
- **Codex copy:** `crates/cas-core/src/sync/agents_md.rs:49-52` generates AGENTS.md from
  CLAUDE.md. It swaps `mcp__cas__` for `mcp__cs__` and nothing else, and handles the
  `claude-only`/`codex-only` markers. It runs only on `cas sync agents-md`.
- **Init-written `cas` skill:** `docs_and_skill.rs:150-216` (`CAS_SKILL`, 2,897 B). It is
  byte-identical in 31 of 32 installed copies (3 Claude homes + 28 projects); `~/Richards LLC`
  differs (2,891 B).

| Installed copy | Block vs template |
|---|---|
| `~/CLAUDE.md` | current |
| `cas-src/CLAUDE.md` (tracked) | current |
| `~/Petrastella/CLAUDE.md` | **stale**: no bug-routing line (mtime 2026-09-04) |
| 40 CLAUDE.md files at depth ≤ 2 under ~ | 32 current, **6 stale** (`~/Petrastella`, `gabber-qa-epic`, `gabber-studio-wt-utf8`, `~/Richards LLC` (pre-ToolSearch), `~/ai/hermes`, `~/proofs/psc-deploy`), 2 without a block |
| `cas-src/AGENTS.md` (tracked) | block current with `mcp__cs__`; **rest stale**: `cas sync agents-md --check` → "1 AGENTS.md file(s) are stale". The CI docs-only paragraph from 5f6c22ba7 (2026-09-18) is missing. |

The 32 "current" descendant blocks were refreshed on 2026-09-05 (e.g. `ozer/CLAUDE.md` mtime
17:36), even though `update_claude_md()` returns early whenever an ancestor has a block. **The
writer that bypassed the ancestor skip is unverified.** Separately, `update/preview.rs:13-45`
(`compute_claude_md_change`) has no ancestor check, so `cas update --dry-run` can promise a
CLAUDE.md change that the apply step will not make.

## Findings (severity-ranked)

`always` = paid every turn; `per-invoke` = when a skill or agent loads; `on-demand` = when read.

### P0

| # | Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|---|
| F1 | P0 | per-invoke (description always) | `cas-cli/src/cli/init/docs_and_skill.rs:177` (`CAS_SKILL`, installed in 32 places) | The init-written `cas` skill tells agents to pass `start` ("Set to true to start immediately (RECOMMENDED)") on `task action=create`. That parameter does not exist, and the request type rejects unknown fields, so an agent that follows the recommendation gets a validation error. | `crates/cas-mcp/src/types.rs:141-144` has `#[serde(deny_unknown_fields)] pub struct TaskRequest` and no `start` field; the live MCP schema is `"additionalProperties": false` with no `start` property. 09-02 noted the param only as "found in no other skill". | Delete the line. Better still, reduce `CAS_SKILL` to a pointer, as 09-02 proposed: "Track work with `mcp__cas__task` (see cas-task-tracking), memory with `mcp__cas__memory`, context with `mcp__cas__search`." | −600 per-invoke |
| F2 | P0 | always (Claude, cas-src main checkout); hook surfacing | `cas-src/.claude/rules/cas/rule-002.md:5`; store rule `rule-002` (Proven, Paths: all, surfaced 49) | A Gabber Studio rule, "ALWAYS cut new branches … from `staging`, never from `develop` … overrides any tooling default", is a proven, unconditional rule in **cas-src's** store and rules directory. cas-src has no `staging` branch (`git branch -r` shows `origin/main`, `origin/develop`), and its epics branch from `main`. The rule tells any supervisor in the main checkout to base work on a branch that does not exist here. | `mcp__cas__rule action=show id=rule-002`; `action=list` shows it as the **only** active rule. The file mtime is 2026-09-24 20:18, so it was re-synced. Likely cause: a Gabber session writing through `CAS_ROOT=…/cas-src/.cas` (this session's env carries that override). The cause is not verified. | Move the rule to gabber-studio's store (or scope it `paths:` to that repo) and delete it from cas-src. Add a sync guard that refuses rules whose text names another registered project. | −91 always |

### P1

| # | Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|---|
| F3 | P1 | always (Grok) | `CLAUDE.md` (10,384 chars) + `AGENTS.md` (8,775 chars) | Grok loads **both** files from every directory between the repo root and the cwd. It gets the same guide twice with three different tool prefixes: `mcp__cas__` in CLAUDE.md, `mcp__cs__` in AGENTS.md, and neither is Grok's own (`cas__`, per the grok mirrors). It also caps each file at 10,000 characters, so the last 384 characters of CLAUDE.md are cut. That removes the tail of "Releases and harness diaries → Slack (mandatory)" and its rubric link (`CLAUDE.md:146-149`). | `grok inspect --json` (grok 1.0.41) → `projectInstructions` = both files, 2,602 + 2,198 approxTokens. `~/.grok/README.md:1556-1567`: the filenames include `Claude.md` and `AGENTS.md`, and "Each file is capped at 10,000 characters (truncated…)". Char offset 10,000 falls mid-sentence at "…two distinct top-level posts (user and dev). A d". | Make one canonical file. Following current Claude guidance (code.claude.com/docs/en/memory: "create a CLAUDE.md that imports it"): AGENTS.md is canonical and harness-neutral (plain tool names, per-harness prefix table), and CLAUDE.md is `@AGENTS.md` plus the `claude-only` lines. Keep each file under 10,000 chars and add a `--check` size gate. | −2.1 k always (Grok) |
| F4 | P1 | always (Codex, Grok) | `docs_and_skill.rs:10-13` projected into `AGENTS.md:6-7` | The block's bootstrap is Claude Code syntax: "`ToolSearch(query="select:mcp__cs__task,…")`" and "DO NOT USE BUILT-IN TOOLS (TodoWrite, EnterPlanMode)". Codex and Grok have no tool with those names, so a literal-minded model spends turns trying to call `ToolSearch`. The generator only rewrites the prefix. | `agents_md.rs:50` (prefix swap only). OpenAI documents tool search as a Responses API `tool_search` tool, not a `ToolSearch(query=…)` call. | Put the ToolSearch and TodoWrite lines in `<!-- claude-only:start -->` inside the template. Give Codex/Grok one line: "CAS tools are `<prefix>task/memory/search`; call them directly." | −75 always (Codex, Grok) |
| F5 | P1 | always (every Claude session; ×3 in Petrastella projects) | `docs_and_skill.rs:8,10` | Two shouted lines (`# IMPORTANT: …`, `**DO NOT USE BUILT-IN TOOLS (TodoWrite, EnterPlanMode)…**`) forbid a tool that current models do not have. Claude Code ≥ 2.1.268 exposes `TodoWrite` and the `Task*` tools by default only on Claude 3.x/4.x models, so on Opus 5.x / Fable neither exists unless opted in. `EnterPlanMode` is for planning, not task tracking (09-02 P2, unfixed). | code.claude.com/docs/en/agent-sdk/todo-tracking ("available by default only on Claude 3.x models, Opus 4 through 4.7…"); this session's tool list has no TodoWrite or TaskCreate. Rubric Axis 4: shouting in always-loaded text = P1. | "Track work in Cassy (`mcp__cas__task`), not in harness-local todo lists; Cassy tasks persist across sessions." Drop the H1 shout. | −40 always ×3 |
| F6 | P1 | always (Claude) | 38 CLAUDE.md files under ~ carrying a block while `~/CLAUDE.md` has one; 6 of them stale | The ancestor skip (`docs_and_skill.rs:37-93`) stops new duplicates but never removes or refreshes old ones. A Petrastella project session therefore loads the block three times, including the stale `~/Petrastella/CLAUDE.md` copy. That copy has no bug-routing line, so two versions of the directive sit side by side. | Survey table above. The `~/Petrastella/CLAUDE.md` file contains nothing but the stale block. | `cas update` removes a descendant managed block when an ancestor carries the current one, deleting the file if it is left empty. Apply the same check in `preview.rs`. One-off: delete `~/Petrastella/CLAUDE.md`. | −600 always per Petrastella session |
| F7 | P1 | always (Codex, Grok) | `AGENTS.md` (tracked) vs `CLAUDE.md` | The generated AGENTS.md has been stale since 5f6c22ba7 (2026-09-18, CI docs-only routing), and nothing enforces regeneration. `agents_md_sync_test.rs` tests the command, not this repo's files. | `cas sync agents-md --check` exits non-zero ("1 AGENTS.md file(s) are stale"). `git log -S'Docs-only' -- CLAUDE.md` → 5f6c22ba7 touched CLAUDE.md only. | Add `cas sync agents-md --check` to Docs Lint, or make F3's single canonical file remove the generator. | 0 |
| F8 | P1 | always (Codex, Grok) | `CLAUDE.md:20-30` codex-only block → `AGENTS.md:22-27` | "Codex does not support Claude hooks" is false twice over. Codex has hooks (developers.openai.com/codex/hooks: SessionStart, PreToolUse, PostToolUse, UserPromptSubmit, Stop…), and CAS itself installs Codex PreToolUse/PostToolUse hooks (`.codex/hooks.json`, `~/.codex/hooks.json`, since cd6b3a8c0 2026-08-11). The PreToolUse guard that denies cargo is exactly such a hook. | File contents quoted. | Replace with: "Codex runs CAS PreToolUse/PostToolUse hooks on Bash (cargo is denied to workers); the supervisor owns verification." Hand to L3: Codex outside cas-src receives **no** Cassy directive. There is no AGENTS.md block downstream (e.g. `ozer/AGENTS.md` has 0 CAS lines, and gabber-studio and pantheon have no AGENTS.md) and no Codex SessionStart hook, although Codex supports one. | ≈0 |
| F9 | P1 | always (agent listing, every harness) | `~/.claude*/agents/`, `~/.codex/agents/`, `~/.grok/agents/`: `code-reviewer.md`, `git-history-analyzer.md`, `issue-intelligence-analyst.md` | Agents no longer in `builtins/agents/` stay installed and listed in every session. `code-reviewer` says "DEPRECATED — replaced by the cas-code-review skill"; that skill was itself retired (09-02), so the pointer dangles. This session's agent list shows all three. | `ls cas-cli/src/builtins/agents/` → 5 agents; the installed homes have 8 (Codex 9). | Prune removed managed agents on `cas update` (same mechanism as L1's refresh P0 and 09-02 epic item 1). | −170 always |
| F10 | P1 | always (all Claude homes + Grok) | `~/.claude*/skills/accounting-client-report/SKILL.md:3` | The description ("Use whenever Daniel asks for a report, summary, or client deliverable for Ben (or any client)…") competes with `cas-html-reports` ("Use when producing a human-readable report…") on every "write a report" request in every project. Its rules also contradict cas-html-reports/ui-craft: "Summary cards at top: number + short title ONLY" vs the anti-KPI-card rule, and HTML-first vs markdown-first. Neither skill names the other. It is also installed in the `support@gabber.studio` Claude home, which leaks client names (Ben Richards, Roark Realty) into that account's listing. | Quoted lines; L4 F3/F4 for the report-skill boundary. | Description: "Use for accounting or tax client deliverables (Richards LLC practice) only; other reports use cas-html-reports, which this skill overrides on layout." Remove it from non-accounting homes. | ≈0 |
| F11 | P1 | per-invoke | `.claude/agents/macos-onboarding-reviewer.md:9,20` | "Current year: 2026. macOS Sonoma (14) and Sequoia (15) are the relevant baselines" is out of date. macOS 27 (released 2026-09-14) is Apple-silicon-only, and macOS 26 Tahoe is the last release for Intel. The reviewer will accept docs that target two-generation-old baselines and will misjudge Intel support. | apple.com/os/macos (macOS 27 compatibility: Apple silicon only); macrumors 2026-09-10. | Replace the hard-coded versions with "Check the current macOS release and its hardware list (WebSearch) before judging baselines", and note the Intel cut-off at macOS 26. | ≈0 |

### P2

| # | Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|---|
| F12 | P2 (highest multiplier) | always (Claude, Grok, Codex in cas-src) | `CLAUDE.md:54-105` | 3.95 KB (≈ 990 tok/turn) of operator build infrastructure loads in every session: the worker no-build policy (enforced by the PreToolUse guard and restated in the spawn prompt and cas-worker), sccache/hardlink seeding, CI-load policy, sccache 0.10.0 measurements, and "Gate evidence: PR #655/run …" (`:64`). Current guidance says to cut "information that changes frequently" and "long explanations" and to keep only lines whose removal causes mistakes. | code.claude.com/docs/en/best-practices (include/exclude table). Rubric Axis 4: don't restate hook-enforced rules. | Keep one line: "Workers never run cargo (hook-enforced); the supervisor builds once per epic — see cas-cli/docs/CONTRIBUTING.md#assembly". Move `:66-105` to CONTRIBUTING.md, or to a path-scoped `.claude/rules/build-ci.md` with `paths: [".github/**","scripts/**","Cargo.toml"]`. | −900 always |
| F13 | P2 | always (Claude) | `CLAUDE.md:127-136` | The "Output hygiene — Ink crash" section says "Until Claude Code ships a fix (cas-97ba tracks)". cas-97ba closed on 2026-04-23 with the pattern **not** bisected (its own notes: "hypothesis … not empirically bisected"). An upstream report records the error as no longer reproducible on 2.1.138; this machine runs 2.1.282. | `mcp__cas__task show cas-97ba`; github.com/anthropics/claude-code/issues/57892. | Re-test on the current Claude Code. If the crash is gone, delete the section; otherwise cite the live tracking task. | −300 always |
| F14 | P2 (09-02 P1 #22 unfixed) | always | `docs_and_skill.rs:22`; `CLAUDE.md:146-149`; `~/CLAUDE.md:18-30` | Three release-announcement rules load together in cas-src: the block line (→ `docs/release-notes/RUBRIC.md`), the repo section (→ `docs/RELEASE_SLACK_RUBRIC.md`, "two distinct top-level posts", diary "exactly three replies"), and the operator's home section ("two threads, each top-level + **one** threaded reply"). They name two rubric files and state different reply counts. The block line is still shipped to every project universally. | File contents. | Block: "If `docs/release-notes/RUBRIC.md` exists, follow it on staging/main merges" (conditional). The repo keeps one rubric pointer. The home section is the operator's own choice (note only). | −30 always |
| F15 | P2 | always | `docs_and_skill.rs:21` vs `CLAUDE.md:142-144` | Bug routing is ambiguous. The block says to file operational bugs in the tracker named by `issues.components.cassy` (= `Richards-LLC/cassy`) "before moving on". The repo says Cassy bugs are fixed "here … via a task … do not report upstream". The block also gives no instruction when `issues.repo` is unset (`env -u CAS_ROOT cas config get issues.repo` in ozer → empty). | `cas config get` outputs. | "In cas-src, a Cassy bug becomes a task here; elsewhere, file it in `issues.components.cassy`. If `issues.repo` is unset, record the bug as a task note." | +15 always |
| F16 | P2 (09-02 P2 unfixed) | always + per-invoke | `docs_and_skill.rs:14-18,150-216` | Five action bullets in the block plus a full manual in `CAS_SKILL` give three different action subsets (with cas-task-tracking). `CAS_SKILL` also shouts ("WHEN TO USE Cassy (ALWAYS)", "IMPORTANT", "RECOMMENDED"), and its description is identity-first ("Coding Agent System - …"). | 09-02 §"The CLAUDE.md directive…"; unchanged since. | Adopt the 09-02 proposed block text; reduce `CAS_SKILL` to a pointer skill ("Use when tracking work, remembering, or searching Cassy context; see cas-task-tracking, cas-memory-management, cas-search"). | −150 always; −600 per-invoke |
| F17 | P2 | always (Codex, Grok) | `~/.codex/AGENTS.md` = `~/.grok/GROK.md` (1,237 B each) | This duplicates the `exa-search` skill body that is installed in the same harnesses, so it is paid on every turn instead of on use. It also points Codex at `~/.claude/skills/exa-search/references/search-api.md` when `~/.codex/skills/exa-search/references/` exists. | Byte-identical (`cmp`). | Reduce to one line: "Web search: use the `exa-search` skill/CLI." | −280 always (Codex, Grok) |
| F18 | P2 | always (all 8 homes) | `~/.claude/skills/exa-search/SKILL.md:3` (520 chars), `:60-64` | The description is over the rubric's 400-char ceiling. The "sync all skill copies after editing" list names 5 locations while 8 copies exist (`~/.claude-daniel@…`, `~/.claude-pippenz@…`, and `~/.claude-support@gabber.studio` are missing). All 8 are identical today; the next appended field learning will drift. `:45` uses "MUST". | md5 of 8 copies; char count. | Description ≈ 200 chars ("Use when searching the web, reading a known URL, or checking current versions, news or docs; run `exa-search`"). Replace the manual copy list with a symlink farm or one sync script. | −80 always |
| F19 | P2 | per-invoke | `~/.claude/skills/accounting-client-report/SKILL.md:13,21,34,36,45` | Shouting on non-safety rules: "ZERO", "ONLY", "NEVER to whole chapter sections", "VISUALLY CHECK". | Rubric Axis 4. | Plain imperatives with reasons; keep the confidentiality footer rule emphatic (it is a safety rule). | 0 |
| F20 | P2 | on-demand (store) | cas-src store: 149 draft rules "Always use descriptive variable names in tests" (created 2026-03-20 → 2026-06-25) | Test fixtures leaked into the real rule store. They are drafts, so they are not synced, but they crowd `rule list_all` (26 of the first 30 rows) and `check_similar`. | `sqlite3 -readonly .cas/cas.db "select count(*) from rules where content like 'Always use descriptive%'"` → 149. | Delete them. The `test-real-store-untouched` make target exists, so confirm it now covers rule writes. | 0 |
| F21 | P2 | always (Claude, cas-src main checkout) | `cas-src/.claude/skills/cas-playwright-debug/SKILL.md`, `cas-seo-expert/SKILL.md` (unmanaged, and also in gabber-studio) | Project-level unmanaged copies shadow or extend the builtins. The `cas-playwright-debug` project copy carries the pre-rewrite description ("Debug and fix Playwright E2E test failures…"), which overrides the managed user-level builtin for supervisors in the main checkout. `cas-seo-expert` is a non-builtin with a `cas-` prefix, so it looks managed. | The installed-vs-source comparison (below) shows `cas-playwright-debug/SKILL.md` differing, with no `managed_by`. | Delete the stale project copy; rename or adopt `cas-seo-expert`. | 0 |

### P3

| # | Surface | file:line | Defect | Fix |
|---|---|---|---|---|
| F22 | — | `cas-cli/src/sync/mod.rs:102-108` | `Syncer::for_two_tier` (global rules → `<~/.config>/.claude/rules/cas-global`) has no caller, and that target is not a directory Claude reads. Global rules sync nowhere. | Delete, or target `~/.claude/rules/` per harness home. |
| F23 | always | `AGENTS.md:32`, `CLAUDE.md:32` | A mid-file `# CLAUDE.md` H1 follows the block; in AGENTS.md this tells Codex it is reading CLAUDE.md. | Use a neutral title ("# cas-src agent guide"). |
| F24 | — | `.claude/settings.json.bak` (tracked) | A backup file is committed beside the live settings. | Remove it from the tree. |
| F25 | on-demand | `~/.claude/skills/exa-search/SKILL.md:58-64` | The "living section — append yours" line invites unreviewed agent edits to a user-level skill shared by 8 homes. | Route learnings through `mcp__cas__memory` and fold them in periodically. |
| F26 | — | `~/.claude-alt/skills`, `~/.claude-pippenz@gmail.com/skills` | No builtins installed (0/138). This is harmless today because every CAS project carries project-level copies (e.g. `~/Documents/Accounting/.claude/skills`, 42 dirs), but it means user-level and project-level installs are double-maintained. | Pick one install level per harness. |

## Installed builtins vs source (per harness home)

Compared byte for byte against `cas-cli/src/builtins/{skills,codex/skills,grok/skills}`
(138 files each):

| Home | Same | Differ | Missing | Differing files |
|---|---:|---:|---:|---|
| `~/.claude/skills`, `~/.claude-daniel@petrastella.io/skills`, `~/.claude-support@gabber.studio/skills` | 134 | 4 | 0 | `cas-image-generate/scripts/generate-image.sh` (installed 2026-08-29), `cas-release-report/scripts/render.py` (09-08), `cas-technical-drawing/scripts/draft.mjs` (09-06), `cas-wizard/template.sh` (08-20) |
| `~/.codex/skills` | 134 | 4 | 0 | same four |
| `~/.grok/skills` | 134 | 4 | 0 | same four |
| `cas-src/.claude/skills` (project) | 133 | 3 | 2 | the scripts above + `cas-playwright-debug/SKILL.md` (unmanaged shadow, F21); `cas-nuxt-playwright` absent (opt-in, correct) |
| `gabber-studio/.claude/skills` (project) | 132 | 4 | 2 | same four scripts |
| `~/.claude-alt/skills`, `~/.claude-pippenz@gmail.com/skills` | 0 | 0 | 138 | not installed (F26) |

Every SKILL.md is current. Every differing file is a non-SKILL.md script, which is exactly
**L1's cross-lane P0** ("bundled non-SKILL.md files never refresh after first install",
`sync_builtin_detailed`). Linked, not re-reported. Grok also resolves the Claude-flavoured
`cas` skill from `~/.claude/skills/cas/SKILL.md` (`grok inspect` skills list), which is the
second cross-lane P0 (Grok resolving `.claude` dirs).

## Items from the 2026-09-02 review: status

| 09-02 item | Status |
|---|---|
| Block `:22` Slack release rule shipped universally (P1 #22) | unfixed (F14) |
| ToolSearch two-step accurate for Claude; keep | still accurate for Claude (tools are deferred in this session); wrong for Codex/Grok (F4) |
| Block `:14-18` duplicates cas-task-tracking; `EnterPlanMode` category error | unfixed (F5, F16) |
| `CAS_SKILL` a fourth manual with a `start=true` param | unfixed; upgraded to **P0** (F1): the param is rejected by `deny_unknown_fields` |
| Retire git-history-analyzer / issue-intelligence-analyst | retired from source; **still installed everywhere** (F9) |

## Currency research (exa-search, 2026-09-25)

- **Claude Code memory:**
  - CLAUDE.md files load in full up the directory tree.
  - Keep each file under 200 lines, and keep only lines whose removal would cause mistakes.
  - Block HTML comments are stripped before injection.
  - `.claude/rules/*.md` without `paths:` load unconditionally.
  - For repos that also have AGENTS.md, use `@AGENTS.md` imports in CLAUDE.md rather than
    duplicating it.
  - MEMORY.md loads its first 200 lines or 25 KB.
  - Sources: code.claude.com/docs/en/memory, /best-practices.
- **Claude Code todo tools:** TodoWrite and the Task* tools are on by default only for Claude
  3.x/4.x models (≥ 2.1.268). Source: code.claude.com/docs/en/agent-sdk/todo-tracking.
- **Codex hooks:** SessionStart, PreToolUse, PostToolUse, UserPromptSubmit, Stop and others
  are supported. Source: developers.openai.com/codex/hooks.
- **Grok 1.0.41:**
  - It reads `Agents.md`/`Claude.md`/`AGENT.md`/`AGENTS.md` from the repo root down to the cwd
    and caps each file at 10,000 characters.
  - It loads `.claude/agents`, `~/.claude/skills` and CLAUDE.md for compatibility.
  - Sources: `~/.grok/README.md:1552-1567,2345-2353` and `grok inspect --json`.
- **macOS 27** (2026-09-14) supports Apple silicon only.
- **Ink `<Box>`-in-`<Text>`:** reported fixed for one trigger by Claude Code 2.1.138
  (anthropics/claude-code#57892).

## Search manifest

| Command | Hits |
|---|---|
| `grep -rn 'USE Cassy FOR TASK' cas-cli/src --include=*.rs` | 3 (template `docs_and_skill.rs:8`, migration check `:121`, preview `update/preview.rs:45`) |
| `grep -rn -e build_cas_section -e update_claude_md cas-cli/src` | 4 callers (init ×2, update, preview) |
| CLAUDE.md survey, depth ≤ 2 under ~ (python) | 40 files: 32 current, 6 stale, 2 no block |
| `CAS_SKILL` vs installed `cas/SKILL.md` | 32 copies: 31 identical, 1 differs |
| `cas sync agents-md --check` | stale: 1 |
| `git log -S'Docs-only' -- CLAUDE.md` | 5f6c22ba7 (CLAUDE.md only) |
| `grok inspect --json` projectInstructions | 2 files (CLAUDE.md, AGENTS.md) |
| `mcp__cas__rule list` / `list_all` | 1 proven (rule-002) / 175 total, 149 duplicate drafts |
| `grep 'pub start' crates/cas-mcp/src/types.rs` (TaskRequest) | 0; `deny_unknown_fields` at :142 |
| installed-vs-source byte compare, 8 homes × 138 files | 4 differing scripts per home; 2 homes empty |
| `ls ~/.claude*/agents ~/.codex/agents ~/.grok/agents` vs `builtins/agents` | 3 retired agents present in every home |
| `md5sum` exa-search copies | 8 identical |
| `grep -c 'CAS' ozer/AGENTS.md`; `ls gabber-studio/AGENTS.md pantheon/AGENTS.md` | 0; absent, absent |
| exa-search queries | 7 (Codex tool_search, Claude todo tools, Ink crash, Claude memory ×2, Codex hooks, macOS 27) |

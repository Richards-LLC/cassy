# Skills & prompts audit 2026-09 — synthesis and proposed fix plan

EPIC cas-1660 · task cas-77915 · 2026-09-25 · calm-owl-92 · findings only, no code edited, no cargo.
Inputs (this directory): `L1-rubric.md`, `L1-findings.md` (cas-63c5), `L3-findings.md` (cas-988a),
`L4-findings.md` (cas-a4d8). **Pending:** L2 (cas-3e02), L5 (cas-9233), L6 (cas-ea56) — merged in as they land.
Code baseline `4836e56f7` (v3.31.0); the epic tip adds only audit docs (`git diff --stat 4836e56f7..HEAD`: 4 docs files).

Row references: `L1#n` = L1 ranked-findings row n; `L3 P0.n` / `L3 P1.n` / `L3 P2.n` = n-th row of that
severity in L3, counting only rows of that severity (the P3 provenance row inside the L3 P2 table is skipped); `L4 Fn` = L4 finding n. "✔ verified" = spot-checked against the code for this synthesis.

## 1. Verdict (one screen)

The shipped skills read well sentence by sentence; what fails is the machinery around them.

- **Agents are handed calls that fail as written (6 P0s).** Claude workers are told to call Codex-spelled
  `mcp__cs__` tools; suggested `message` calls omit the mandatory `summary`; local-merge workers are told to
  `git push` (hook-denied); verifier guidance omits `dispatch_id`; `cas-release-report` ignores the
  product's own `cas release report` command. Each costs a failed call plus a recovery turn, every time.
- **Updates never reach installed copies.** Skill scripts/examples freeze at first install
  (`sync_builtin_detailed`), retired agents are never pruned, and a shipped exemplar leaks operator e-mails
  and cost figures into every downstream project — and, because of the first defect, deleting it at source
  won't remove it from installs.
- **Always-loaded text is over the harness limits the code itself documents.** Worker SessionStart
  11.8 KB and supervisor 13.2 KB exceed Claude Code's 10,000-char hook cap (model sees a ~2 KB preview);
  the `coordination` tool description (2,834 chars) is cut at 2,048; ~11 KB of the MCP schema is rmcp
  boilerplate; supervisor-only params load into every worker.
- **The per-harness spelling model does not survive contact with the harnesses.** Grok and OpenCode load
  `.claude/skills` (Claude spelling), AGENTS.md ships the Codex spelling with Claude's ToolSearch syntax,
  Codex ignores `.md` agents entirely — while the repo maintains 414 embedded skill files and a 1,588-line
  drift test to keep three spellings in sync.
- **Rules drift because they are hand-copied.** Three worker contracts, four verification-timeout texts,
  five commit-receipt recoveries, two request structs, three form-choice tables, two token vocabularies.

**Ask of the operator:** approve work packages WP1–WP5 now (correctness + budget, no architecture change),
and rule on the decide-first list (§5) before WP8–WP10 start. Estimated always/per-session savings after
WP2+WP3: ~2.3k tokens per worker SessionStart made *deliverable*, ~4.5k tokens per worker session in MCP
schemas, plus the elimination of the recurring failed-call turns.

## 2. Master findings table (deduplicated)

Severity is the highest any lane gave. Every lane P0/P1 appears here or is marked a duplicate (§2.3).

### 2.1 P0

| ID | Sev | Finding | Lane refs | Evidence (strongest) | ✔ |
|---|---|---|---|---|---|
| M01 | P0 | Claude workers receive Codex `mcp__cs__` tool names in every assignment, stall nudge and reply footer — prefix taken from the session-wide `worker_cli`, not the recipient | L3 P0.1 | `ui/factory/director/prompts.rs:1357-1393,744`; caller `app/mod.rs:1362`; live assignment text in this worker's transcript | ✔ |
| M02 | P0 | Every suggested `coordination action=message` template omits `summary`, which the handler requires; schema does not mark it required | L3 P0.2 | `agent_search_system/message.rs:637-645`; templates `prompts.rs:744,1388`, `crates/cas-pty/src/pty.rs:20,41,315,1404` | ✔ |
| M03 | P0 | MERGE REQUIRED / MERGE REALITY remediation tells `local_merge` workers to `git push origin`, which PreToolUse denies | L3 P0.3 | `close_ops.rs:10918,11786` vs `pre_tool.rs:224-229` | ✔ |
| M04 | P0 | Direct-verdict guidance `verification action=add` omits the required `dispatch_id` (self-contradicting at 6731/6761) | L3 P0.4 | `close_ops.rs:6381,6731`; `verification_tools.rs:337-345` | ✔ |
| M05 | P0 | Skill files that are neither `SKILL.md` nor `references/*` (8 scripts/examples) freeze at first install; SKILL.md text updates, scripts don't | L1#1 | `builtins.rs:2318-2339` (`SkippedNotManaged`), `:2534-2549` (references-only owner check); installed `cas-wizard/template.sh` ≠ source | ✔ |
| M06 | P0 | `cas-html-reports` before/after exemplar is a real operator report: 3 e-mail identities, `~/.codex-support@…` paths, $ cost figures, task ids — shipped to every install ×3 flavours (regression of 09-02 P1 #8) | L4 F1 (+F18 milder) | `cas-html-reports/references/examples/before-after/rubric-review-{before,after}.html` (7+7 identity hits) | ✔ |
| M07 | P0 | `cas-release-report` never mentions `cas release report <v> [--pdf]`; prescribes a hand pipeline instead; 22/29 reports since v3.22.1 skip the skill's brief + QA steps | L4 F2 | `grep -c 'release report' SKILL.md` = 0; `cas release report --help` exists in 3.31.0 | ✔ |

### 2.2 P1

| ID | Sev | Finding | Lane refs | Evidence | Theme |
|---|---|---|---|---|---|
| M08 | P1 | SessionStart over the 10K hook cap for every factory role (worker 11,820 B, supervisor 13,181 B); Knowledge + Handoff protected by omission from the degradable list | L3 P1.1 | `session_budget.rs:91-112`; hook stderr; 18 persisted `hook-*-additionalContext.txt` files | T3 |
| M09 | P1 | Claude workers on non-default config dirs likely get **no** SessionStart: custom-profile fallback is supervisor-only; cas-src `sessions` table has no row since 2026-09-11 | L3 P1.3 | `app/mod.rs:2591-2614`; `session_start_fallback.rs:20-33` | T3/T4 |
| M10 | P1 | `coordination` description 2,834 chars, Claude Code cuts at 2,048; lost tail contradicts the visible `force` param | L3 P1.4 | `service/mod.rs:541`; code.claude.com/docs/en/mcp; claude-code#81268 | T3 |
| M11 | P1 | Knowledge index injects the alphabetically-first 11/148 pages (gabber PostHog pages in cas-src) regardless of task | L3 P1.2 | `build_start.rs:80-135` | T3 |
| M12 | P1 | Role-mismatch banner hardcodes `mcp__cs__coordination` after the remap pass → Claude supervisors told to call a Codex tool | L3 P1.5 | `handlers_session.rs:11` | T1 |
| M13 | P1 | WORKTREE MERGE JAIL demands a non-existent `worktree-merger` agent; unjail matches only `Task`, not `Agent` (default-off feature) | L3 P1.6 | `pre_tool.rs:536,566-571`; `close_ops.rs:6862` | T5 |
| M14 | P1 | MERGE REALITY steps contradict the epic-branch merge model ("open a PR targeting {parent}") and give incomplete commands | L3 P1.7 | `close_ops.rs:11772` vs `:10888/10897` | T4/T5 |
| M15 | P1 | `USAGE_REMINDER` `<IMPORTANT>`/"PROACTIVELY" in always-loaded text; restates CLAUDE.md block + brief; claims "semantic" search vs BM25 | L3 P1.8 | `cas-core/src/hooks/context/mod.rs:520-541` | T3/T7 |
| M16 | P1 | Grok resolves all 42 CAS skills from `.claude/skills` (Claude spelling) in any checkout that has them | L1#2 | `grok inspect --json` in cas-src | T1 |
| M17 | P1 | OpenCode loads Claude-spelled skills; the `cas_` projection is never written to disk | L1#3 | `builtins.rs:1831-1860,2819`; `opencode debug skill` | T1 |
| M18 | P1 | Codex custom agents are TOML; every installed `.codex/agents/*.md` is inert; `factory-supervisor.md` has no consumer | L1#4 | developers.openai.com/codex/subagents; codex 0.156 strings | T1/T2 |
| M19 | P1 | Retired managed agents (`code-reviewer`, `git-history-analyzer`, `issue-intelligence-analyst`) never pruned from installs; still in every Claude Agent list | L1#5 | `builtins.rs:3192-3237` (skill prune only) | T2 |
| M20 | P1 | `disallowed-tools` documented as a session guard; it is turn-scoped, Claude-only; cas-worker's TodoWrite ban lapses on the first supervisor message | L1#6 | claude 2.1.282 schema string; `cas-worker.md:5-7` | T5 |
| M21 | P1 | AGENTS.md generated in Codex spelling with Claude ToolSearch syntax and caps; Grok gets it twice (+CLAUDE.md); OpenCode reads it with the wrong prefix | L1#7 (+L3 P1.8 duplication) | `cli/sync/agents_md.rs`; `cli/init/docs_and_skill.rs:10-23` | T1/T7 |
| M22 | P1 | Release report triggers both `cas-html-reports` and `cas-release-report` with different heroes/layouts | L4 F3 | `cas-html-reports/SKILL.md:3,61`; `report-types.md:37,126-138` | T4 |
| M23 | P1 | Three contradictory form-choice sources (pie, KPI cards) | L4 F4 | dataviz `SKILL.md:24-29` vs `form-vocabulary.md:19,31,34` vs `presentation-rules.md:73,79,104-110` | T4 |
| M24 | P1 | Public-surface QA gates require `scripts/visual-qa.mjs` / `terminal-qa.mjs`, which ship only in cas-src | L4 F5 | not in any `builtins.rs` entry | T2/T5 |
| M25 | P1 | Two token vocabularies with no mapping (DESIGN.md roles vs Petrastella roles) | L4 F6 | `design-spec/SKILL.md:66-76` vs `technical-contract.md:57-63` | T4 |
| M26 | P1 | design-spec frontmatter diverges from the public DESIGN.md spec; skill claims no validator exists (`@google/design.md lint` does) | L4 F7 | google-labs-code/design.md spec | T7 |
| M27 | P1 | `generate-image.sh` cannot request aspect ratio/size; playbook asks for 16:9, 2K, A4, 1200×630 | L4 F8 | `generate-image.sh:146` | T5 |
| M28 | P1 | Descriptions over the ceiling: cli-craft 514, ui-craft 428 (>400 = P1); 5 skills >250 | L4 F9 = L1#8 | char counts | T3 |

### 2.3 Duplicates merged (not separate rows)

| Reported as | Merged into |
|---|---|
| L1#8 (descriptions >250) | M28 (L4 F9 carries the P1 cases) |
| L1#9 / L4 F20 (`managed_by` top-level key) | P2 row M40 |
| L1#11 / L3 P3 row "Codex prompts forbid /cas-start…" | P2 row M41 |
| L3 P1.8 duplication with CLAUDE.md block | M15 + M21 (same always-loaded directive, two carriers) |
| L4 F18 (milder operator names) | M06 remediation scope |
| L3 P3 row "Internal ticket ids in agent text" / L4 F18 ids | M42 |
| Cross-lane P0 "Grok in cas-src resolves `.claude/skills`" (supervisor note) | M16 |
| Cross-lane P0 "non-SKILL.md files never refresh" (supervisor note) | M05 |

### 2.4 P2 / P3 (grouped; full detail in lane reports)

| ID | Sev | Group | Lane refs | Δ tokens (est.) |
|---|---|---|---|---|
| M30 | P2 | MCP schema boilerplate (`default:null`, `nullable`, non-standard `format`) = 16.4% of `tools/list` | L3 P2.1 | −2,860 total; −1,560 for the 4 always-selected tools |
| M31 | P2 | Supervisor-only params in worker-loaded `coordination` (8,330 B) and `task` (4,645 B) | L3 P2.2 | −2,000…−3,000 per worker session |
| M32 | P2 | Action list prose ≠ `action` param list (task omits `request_changes`/`reset`; coordination omits `epic_status`, `server_*`); `action` not an enum | L3 P2.3 | −300…−500 |
| M33 | P2 | `FactoryRequest` vs `CoordinationRequest` hand-copied descriptions, 18/36 differ; colliding field meanings | L3 P2.4 | 0 (drift) |
| M34 | P2 | Ambient recall keys on envelope words (`director`, `color`, `green`), re-injects the just-read task; provenance noise | L3 P2.5, P3 | −400…−800 per turn |
| M35 | P2 | Worker rules delivered twice (8 KB skill at SessionStart + 4 KB contract) and three drifted contract copies; `remind` template missing `remind_message`; push vs local_merge; runtime contract labelled `agent-authored` | L3 P2.6–P2.10 | −790 per spawn |
| M36 | P2 | Close-gate texts: MERGE REQUIRED 2.85 KB of internals; VERIFICATION REQUIRED addresses 4 roles + fenced echo; 4 timeout variants; 5 receipt-recovery copies; vague verifier-authority denials; impossible remedies (`drain_lifecycle_outbox`, Neon self-branching); platform proof before worker deferral | L3 P2.11–P2.18 | −1,000+ per rejection cycle |
| M37 | P2 | session-learn classifier ships the whole SKILL.md (maintainer sections, tool instructions to a tool-less call) | L3 P2.19 | −1,200 per Stop (opt-in) |
| M38 | P2 | Design/report skills: 22.6 KB token JSON read per render; body restates contract; 221 KB exemplar reads; pasted PDF programs; unwired providers; missing TOCs; dataviz vs bundled `dataviz` conflict rule; first step at line 41 | L4 F10–F17, F19 | −4.9k per render; up to −40k per exemplar-following run |
| M39 | P2 | House standard (`cas-writing-for-agents`) stale vs 2026 guidance: no per-harness frontmatter matrix, no description budget, no model-era wording rules, "≤80 lines" contradicts 20/41 skills | L1#12, §2 | +250 per invoke (on-demand) |
| M40 | P2 | Top-level `managed_by: cas` non-portable (claude.ai / Skills API reject) → `metadata.managed_by` | L1#9, L4 F20 | +2 per invoke |
| M41 | P2 | Dead prohibitions of `/cas-start`, `/cas-context`, `/cas-end` | L1#11, L3 P3 | −25 always (Codex spawn) |
| M42 | P3 | Ticket ids / GH numbers / operator names in agent-facing text | L3 P3, L4 F18 | small |
| M43 | P2 | `disable-model-invocation` ignored by Codex/OpenCode (opt-in skills listed there) | L1#10 | −90 always (Codex/OpenCode) |
| M44 | P3 | Misc polish: test comment 1,024 vs 1,536; Grok `/release-notes` collision; reference frontmatter; `~/.codex/skills` deprecated path watch; `draft.mjs` comment; Tailwind colors in examples; `v1beta`; doubled `\\`; empty "## Quick ..." heading; stale `cli`/`model` examples; server `instructions` text | L1#14–17, L4 F21–F28, L3 P3 | small |

## 3. Themes (root causes spanning lanes)

**T1 — Tool-prefix model.** The harness prefix is baked into text at authoring or render time and chosen
from the wrong source: session-wide harness instead of the recipient (M01), a literal after the remap pass
(M12), the harness that *wrote* a directory instead of the harness that *reads* it (M16, M17, M21). The
three-spelling catalog (414 files, 1,588-line drift test) guarantees source parity but not delivered
parity. Root fix is either per-recipient rendering everywhere (keep spellings) or a prefix-neutral catalog
that names tools by bare name and states the prefix once in role guidance (L1#13) — decision D1.

**T2 — Install lifecycle is add-only.** Sync overwrites only files carrying frontmatter it can match
(M05), prunes only `cas-*` skill dirs (never agents, never non-`cas-` builtins: M19), and the ledger covers
only `references/`. Consequences cascade: the operator-data exemplar (M06) and every script fix
(M24, M27) cannot reach existing installs until this is fixed. No test compares an installed copy to the
catalog. **WP4 must precede or ship with WP5/WP7.**

**T3 — Always-loaded budget has no aggregate owner.** Components are budgeted (8 KB worker skill, 8 KB
supervisor skill) but assembled payloads are not: SessionStart (M08, M11), MCP descriptions (M10, M30–M32),
USAGE_REMINDER + CLAUDE.md block + AGENTS.md + brief restating the same directive (M15, M21), skill
descriptions (M28). The harness caps are external and silent (10,000-char hook cap, 2,048-char
description cap, Codex 8,000-char listing fallback). Needed: one test per surface asserting the *assembled*
size against the real cap.

**T4 — Rules maintained as hand copies.** Worker contracts ×3 + skill (M35), close-gate texts (M36),
request structs (M33), action lists (M32), form tables ×3 (M23), token vocabularies ×2 (M25), report
heroes ×2 (M22). Parity tests check markers, not meaning. Fix pattern: one renderer/helper or one owning
file per rule; others point to it.

**T5 — Prose not tied to what the code accepts.** Suggested calls omit required params (M02, M04, remind),
name forbidden actions (M03), non-existent agents/commands (M13, M41), unshipped scripts (M24), or
guards that aren't guards (M20); a skill ignores the CLI that implements it (M07). Fix pattern: generate
suggested calls from the schema (or test them against it), and route prohibitions through the hook that
enforces them instead of prose.

**T6 — Operator data leaks into shipped artifacts.** M06, M42; the 09-02 de-operator-ise item regressed.
Needs a lint (e-mails, `/home/<user>`, `~/.codex-*`, `cas-[0-9a-f]{4}` in shipped builtins).

**T7 — Wording lags current model guidance.** Emphasis in always-loaded text (M15, M21), verification
scaffolding, conflicting rules that stall GPT-6 (M23, M35 contradictions). Mostly handled by the house
standard rewrite (M39) plus targeted edits.

## 4. Proposed fix plan (work packages)

Risk classes: **R-build** = Rust change, needs supervisor assembly build + full test; **R-pins** = edits
`cas-worker.md`/`cas-supervisor.md` or tool/session text pinned by tests; **R-docs** = markdown only
(Docs Lint route), still subject to drift/description tests when under `cas-cli/src/builtins`.

Pin set **P8** (from the 3.17.3 lesson): `issue_intake_directive_test`, `factory_codex_skill_guardrails`,
`session_start_issue_triage_test`, `builtin_doc_hygiene_test`, `builtin_flavor_drift_test`,
`builtin_skill_description_test`, `--lib builtins`, `--lib cli::factory::parity`.

| WP | Scope (master IDs) | Main files | Risk | Tests affected / to add | Est. savings | Depends on |
|---|---|---|---|---|---|---|
| **WP1 Envelope & remediation correctness** | M01, M02, M03, M04, M12, M13, M14, remind/`push` rows of M35 | `director/prompts.rs`, `app/mod.rs`, `cas-pty/src/pty.rs`, `close_ops.rs`, `handlers_session.rs`, `pre_tool.rs`, `types/ops_secondary.rs` (`summary` desc) | R-build, R-pins (contract markers in `pty.rs`, `factory_codex_skill_guardrails`, `--lib cli::factory::parity`) | Director prompt tests; add: every suggested `…action=message` literal contains `summary=`; prefix-per-recipient test (Claude worker in Codex-default session) | Eliminates ≥1 failed call + recovery turn per assignment / rejection (~300–800 tok each) | — |
| **WP2 SessionStart fits the cap** | M08, M11, M15, overview-warning + session-line rows (L3 P2/P3) | `session_budget.rs`, `cas-core/.../build_start.rs`, `cas-core/.../mod.rs` (USAGE_REMINDER), `handlers_session.rs` | R-build, R-pins (`session_start_issue_triage_test`, `session_start_memory_hygiene_test`, `test_worker_guidance_under_session_start_budget`, `test_supervisor_guidance_under_8kb`, session_budget unit tests) | Add assembled-payload ≤ 9,216 B test per role (worker, supervisor, codex worker) on a realistic fixture | Worker −2.6 KB, supervisor −4 KB, USAGE_REMINDER −0.7 KB; guidance actually delivered (~2k tok regained) | — |
| **WP3 MCP schema diet (no split)** | M10, M30, M32, M33, stale-value/ticket rows of M42/M44 | `service/mod.rs` tool descriptions, `crates/cas-mcp/src/types*.rs`, `list_tools` post-process | R-build (no known text pins on tool descriptions — add them) | Add: every tool description ≤ 2,048 chars; `action` enum == dispatch table; schema has no `nullable`/`default:null` | ~−2.9k tok total schema; −1.6k for the 4 always-selected tools | — (D2 extends it) |
| **WP4 Install sync & prune** | M05, M19, non-`cas-` prune gap, ledger glob, install-parity doctor check | `builtins.rs` (`is_reference_owned_by_managed_skill`, prune fns), `scripts/gen-builtin-reference-history.sh`, `reference-history.json`, doctor | R-build | `--lib builtins` sync tests; add installed-vs-catalog parity test; ledger regeneration | −120 always (retired agent descriptions); correctness for every later skill fix | — (blocks WP5 install effect, WP7 script fixes) |
| **WP5 Operator-data purge + lint** | M06, M42 | delete `before-after/rubric-review*` ×3 flavours; `builtins.rs` registrations; exemplar renames (L4 F18); runtime ticket-id strings | R-build (registration removal) + R-docs | `builtin_flavor_drift_test`; add operator-data lint test over shipped builtins | −42.9k on-demand; −170 KB ×3 binary | WP4 (to prune installed copies) |
| **WP6 Close-gate message helpers** | M36 | `close_ops.rs`, `pre_tool.rs`, `supervisor_push.rs`, `neon_sql_guard.rs`, `stale_close_guard.rs` | R-build | Close-ops message tests (substring pins on MERGE REQUIRED / VERIFICATION texts) | −1k+ per rejection cycle | WP1 (shares remediation text) |
| **WP7 Design & report skills coherence** | M07, M22–M27, M38 | `cas-release-report`, `cas-html-reports`, `cas-ui-craft`, `cas-dataviz`, `design-spec`, `cas-image-generate`, `cas-cli-craft` (+ ship `visual-qa.mjs`/`terminal-qa.mjs`, `tokens.css`) | R-docs; R-build for new shipped files (registration) | `builtin_flavor_drift_test`, `builtin_skill_description_test`, `cas_image_generate_skill_test`, design-token parity test (`builtins.rs:5159-5264`) | −4.9k per render; −75 always; up to −40k per exemplar run | WP4 (scripts reach installs); D5, D7, D8 |
| **WP8 House standard & portable frontmatter** | M20, M28, M39, M40, M41, M43, M44 (L1 polish) | `cas-writing-for-agents/SKILL.md`, all `SKILL.md` frontmatter, `cas-worker.md` (`disallowed-tools`), `builtin_skill_description_test.rs` | R-docs + R-pins (P8: description test, doc hygiene, flavor drift; `cas-worker.md` edits hit worker 8,000 B cap) | Extend description ≤250 cap to all skills; `is_managed_by_cas` accepts `metadata.managed_by` | −170 always (descriptions) ×harnesses; −90 Codex/OpenCode | D1 (wording of tool names) |
| **WP9 Harness projection** | M16, M17, M18, M21, L1#13 | `builtins.rs` sync paths, `cli/sync/agents_md.rs`, `cli/init/docs_and_skill.rs`, codex agents (TOML or removal) | R-build, large (drift test redesign if D1 = neutral) | `builtin_flavor_drift_test` (1,588 lines) rewrite or extension; `factory_parity_test`; AGENTS.md sync tests | −450 always for Grok (duplicate block); −60 per harness; −2× catalog maintenance if neutral | **D1, D3, D6** |
| **WP10 Worker/supervisor startup single source** | M09, M35 (contract ×3 → one renderer; skill vs contract dedupe), coordination split if D2 | `cas-pty/src/pty.rs`, `app/mod.rs` fallback, `cas-worker.md`, (split: `ops_secondary.rs`, `service/mod.rs`, every skill naming coordination actions) | R-build, R-pins (P8 full) | Contract marker tests; `factory_codex_skill_guardrails`; add "SessionStart fired" telemetry | −790 per spawn; −2–3k per worker session if split | **D2, D4**; WP2 |

**Suggested order.** Wave A (parallel, independent): WP1, WP2, WP3, WP4. Wave B: WP5 (after WP4), WP6
(after WP1), WP8. Wave C after decisions: WP7 (D5/D7/D8), WP10 (D2/D4), WP9 (D1/D3/D6). One assembly build per
wave; WP2 and WP8 both touch P8-pinned text — land them in the same wave only with the full P8 run in one
pass (see memory "skill-text-pins").

## 5. Decide first (operator)

| # | Decision | Options | Recommendation | Gates |
|---|---|---|---|---|
| D1 | Tool naming in shipped text | (a) keep three spellings, fix delivery per recipient/harness; (b) **prefix-neutral catalog**: bare tool names in skills, prefix stated once in role guidance | (b) — the spellings already fail to reach Grok/OpenCode; retires ~276 embedded twins and most of the drift test. Needs a one-page "tool naming" rule in role guidance. | WP8 wording, WP9 |
| D2 | Split `coordination` | (a) keep one tool, trim; (b) worker-facing `coordination` + supervisor `factory` tool | (b) — worker sessions stop loading ~8 KB of spawn/worktree/db params; lets annotations be honest. Breaks every skill line naming supervisor actions (mechanical). | WP10, WP3 extension |
| D3 | AGENTS.md projection | (a) Codex-only spelling (today); (b) prefix-neutral, no ToolSearch line, plain imperatives; (c) per-harness files | (b) — AGENTS.md is read by Codex, Grok and OpenCode; one neutral text avoids Grok's double load. | WP9 |
| D4 | Canonical carrier for worker startup rules | SessionStart skill body vs launch brief vs Skill tool on demand | Decide after WP2 telemetry answers M09. If SessionStart is unreliable for custom profiles, make the brief canonical and shrink SessionStart to identity + inbox. | WP10 |
| D5 | `cas-release-report` vs `cas release report` | CLI-first skill (brief + QA as craft layer) vs declare CLI reports exempt | CLI-first; practice already follows the CLI. | WP7 |
| D6 | Codex agents | Emit TOML agents vs stop installing `.md` for Codex | Stop installing (they're inert) unless Codex supervisors should delegate natively. | WP9 |
| D7 | QA gates downstream | Ship `visual-qa.mjs`/`terminal-qa.mjs` in skills vs make receipts cas-src-only | Ship them (+81 KB ×3) — otherwise downstream can't meet the floor. | WP7 |
| D8 | Token schema | Adopt public DESIGN.md spec keys + Petrastella roles mapping vs keep house schema | Adopt spec keys with a `maps:` note. | WP7 |

## 6. Lanes pending

L2 (cas-3e02), L5 (cas-9233), L6 (cas-ea56): not yet on the epic branch at the time of this draft; their
P0/P1 rows will be added to §2 with the same dedupe rules.

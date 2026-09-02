# Built-in skills and agent definitions — Fable 5.1 review

**Date:** 2026-09-02 · **Reviewed at:** commit `0489d956` (v3.10.1, `main`) · **Task:** cas-4bd6 · **Author:** kind-lion-55 (Claude Fable 5.1, high effort) · **Status:** review only — no skill was edited

## Verdict

The 35 canonical skill entries and 8 agent definitions are structurally sound and, in most places, technically accurate — but the v3.10.0 removal of the code-review layer, the m248 status migration, and the `supervisor_override` rename were only half-propagated into the markdown, and a handful of always-loaded documents are paying for prose that the harness already enforces. **22 P0 findings** describe content that misleads an agent *today* (wrong parameter names, retired statuses, commands that do not exist, contradictory procedures). **Every one is a small, mechanical fix**; the larger structural work (dedup, description rewrites, opt-in tiering) is P1–P2 and fits one epic of ~20 tasks.

Three numbers to carry away:

- **The verifier is silently broken twice.** Every `task-verifier` verdict template passes `files="…"` but the schema field is `files_reviewed` (never recorded), and its test-first check runs `rg -E` — which is ripgrep's `--encoding` flag — so it errors on every invocation.
- **9,956 bytes of worker guidance is injected verbatim into every worker session against a 9,216-byte SessionStart budget**, so the Ready-Tasks / memories / skills segments are dropped to headings on every spawn.
- **The retired `cas-code-review` skill still surfaces in every session** on this machine because the stale-skill pruner only deletes directories that are *not* `managed_by: cas` — the inverse of what a removed builtin needs.

## Scope and method

| Item | Value |
|---|---|
| Canonical skills reviewed | 35 entries under `cas-cli/src/builtins/skills/` (28 directories + 7 flat `.md` files: cas-search, cas-task-tracking, cas-supervisor, cas-supervisor-checklist, cas-worker, plus 2 in dirs). The brief said 28; the count is 35 once the flat factory guides are included. |
| Agent definitions reviewed | 7 under `cas-cli/src/builtins/agents/` + the Codex-only `codex/agents/factory-supervisor.md` |
| Also reviewed | The CLAUDE.md directive block and project `cas` skill written by `cas init` (`cas-cli/src/cli/init/docs_and_skill.rs:8-22,150-216`) |
| Lines read | 13,108 canonical (incl. references/, scripts, examples) + 2× projections for drift |
| Yardstick | `cas-writing-for-agents/SKILL.md` + `SKILL-MECHANICS.md`; Anthropic skill guidance for Claude 4.x/5 (trigger-phrased description, imperative steps with completion criteria, progressive disclosure, no harness-enforced instructions) |
| Ground truth | MCP dispatch tables in `cas-cli/src/mcp/tools/service/mod.rs` (every `match req.action.as_str()`), param schemas in `cas-cli/src/mcp/tools/types/*.rs`, `cas --help` and every subcommand `--help` on `cas 3.10.1`, `CHANGELOG.md` 3.10.0 entry, `git` and `rg` on the operator box |
| Not reviewed | `.claude/skills/` in the repo root (synced copy); the `.claude/rules/cas/` mirror is generated from proven DB rules by `cas-cli/src/sync/mod.rs`, not from builtins, so it has no static source to score |
| Note | The brief asked to run `cas skill list`; that subcommand does not exist (`cas 3.10.1` → "unrecognized subcommand 'skill'"). Skills are MCP-only via `mcp__cas__skill action=list_all`. CLI claims were verified with `cas --help` instead. |

Scores are 1–5 on four axes: **D** description quality (can a model decide quickly and correctly whether to invoke it), **S** structure/formatting (front-loaded procedure, progressive disclosure earned, no dead weight), **C** content accuracy against the current codebase, **A** agentic effectiveness (evidence before done, stop conditions, handoff, what-not-to-do, would a stronger model do better with less text). Verdicts: keep / revise / merge / retire. Effort: S (<1 h), M (half day), L (day+, multi-file, three mirrors).

## Scorecard

"Loaded" marks the documents whose body is injected into every factory session (`supervisor_guidance()` / `worker_guidance()`, `cas-cli/src/config/access/hooks_traits.rs:53-57,102-106`) or every project's CLAUDE.md; every word there is paid on every turn.

| Skill / agent | Lines (body+refs) | D | S | C | A | Verdict | Effort | Loaded | Headline finding |
|---|---|---|---|---|---|---|---|---|---|
| cas-worker | 150+894 | 4 | 3 | 3 | 3 | revise | L | yes | 9,956 B body exceeds 9,216 B SessionStart budget; cites retired `pending_supervisor_review` |
| cas-supervisor | 63+1,282 | 5 | 4 | 3 | 3 | revise | M | yes | "documented escape hatch" is defined nowhere; `supervisor_override` never named |
| cas-supervisor-checklist | 105 | 4 | 3 | 3 | 3 | revise | M | on demand | Points at a "Pre-flight section in cas-supervisor" that does not exist |
| cas-task-tracking | 31 | 4 | 4 | 2 | 3 | revise | S | no | "Exact list — do not invent others" omits 8 valid actions |
| cas-memory-management | 153+576 | 4 | 2 | 2 | 2 | revise | L | no | Teaches `mode=autofix` as unimplemented (it is live), `--no-overlap-check` (real: `bypass_overlap`), and two CLI commands that do not exist |
| cas-search | 63 | 4 | 4 | 3 | 3 | revise | S | no | "Exact list" omits `history`, `retrieval_*`, `skill_impact` |
| session-learn | 141 | 3 | 2 | 2 | 2 | revise | M | no | Claims the Stop hook embeds this body via `include_str!`; it does not |
| verify-before-claim | 103 | 5 | 3 | 3 | 3 | revise | S | no | Dangling `(see close-gate.md)`; v1 decision narration |
| cas-writing-for-agents | 45+13 | 5 | 3 | 4 | 2 | revise | S | no | The yardstick has no steps or completion criterion, violating its own rule |
| cas-tdd | 39+20 | 3 | 3 | 4 | 3 | revise | S | no | Identity sentence before trigger; NestJS carve-out leaked into a builtin |
| cas-diagnosing-bugs | 62 | 5 | 4 | 4 | 4 | keep | S | no | Names no Cassy tool; otherwise tight |
| cas-codebase-design | 57+17 | 4 | 3 | 4 | 3 | revise | S | no | Two ≤10-line references behind pointers; NestJS leakage |
| cas-domain-modeling | 47 | 4 | 3 | 4 | 3 | merge → cas-codebase-design | S | no | 60 % restates cas-memory-management |
| cas-resolving-merge-conflicts | 17 | 5 | 4 | 4 | 4 | keep | S | no | — |
| cas-to-questionnaire | 16 | 4 | 4 | 4 | 4 | keep | S | no | Correctly user-invoked |
| cas-wizard | 15+24 | 4 | 4 | 4 | 4 | keep | S | no | `template.sh` drifted from its mirrors (24 vs 18 lines) |
| cas-brainstorm | 241+266 | 4 | 3 | 3 | 4 | revise | M | no | `disallowed-tools: Write` forbids the tool Phase 3 requires; hands off to non-existent `/plan` |
| cas-ideate | 176+208 | 4 | 4 | 3 | 4 | revise | S | no | Same `disallowed-tools` contradiction |
| project-overview | 189 | 4 | 4 | 4 | 3 | revise | S | no | No commit step although git history is its freshness signal |
| codemap | 202 | 4 | 4 | 4 | 4 | revise | S | no | Claims Missing blocks worker dispatch; gate fires only on SignificantlyStale |
| design-spec | 157 | 4 | 4 | 3 | 3 | revise | S | no | References the removed "design-review persona"; promises a drift signal that no code emits |
| release-notes | 78+69 | 4 | 4 | 2 | 4 | revise | M | no | cas-src-only transport policy shipped to every project; contradicts its home rubric on reply count |
| cas-github-issues | 230 | 4 | 4 | 4 | 5 | revise | S | no | Lists retired `pending_supervisor_review`; otherwise the sharpest stop conditions in the set |
| cas-html-reports | 124+644 | 4 | 3 | 5 | 3 | revise | M | no | 43 lines of stance before the first step; contract restated twice |
| cas-dataviz | 62+44 | 2 | 4 | 4 | 4 | revise | S | no | Trigger words identical to Claude Code's bundled `dataviz` → double fire |
| cas-image-generate | 72+546 | 3 | 4 | 4 | 4 | revise | S | no | Description leads with the provider; links to a non-shipped dossier |
| cas-servers | 101 | 5 | 4 | 5 | 5 | keep | S | no | Every parameter verified |
| mcp-integration | 215+216 | 4 | 2 | 2 | 3 | revise | L | no | Teaches only `claude mcp`; never mentions `cas mcp add`, `.cas/proxy.toml`, `proxy_*` |
| cas-viktor | 36+53 | 4 | 4 | 4 | 4 | revise | S | no | Never shows the `mcp_execute` call shape |
| cli-routing | 47+104 | 2 | 4 | 3 | 4 | revise | M | no | Operator e-mail in a shipped builtin; source-tree-only links |
| cas-codex-exec | 66 | 5 | 5 | 3 | 4 | revise | S | no | Pins `-m gpt-5.5`; conflicts with cli-routing's recipe |
| fallow | 382+2,978 | 4 | 2 | 3 | 2 | revise + opt-in | M | no | Rules mandate `\|\| true`; no example uses it; 3 k vendored lines ×3 mirrors in every project |
| cas-nuxt-playwright | 228+265 | 2 | 4 | 4 | 3 | revise | S | no | "Opt-in only" by shouting instead of `disable-model-invocation: true` |
| task-verifier (agent) | 409 | 3 | 2 | 2 | 3 | revise | L | per close | `files=` dropped; `rg -E` errors; "VERIFICATION JAIL" phrase never emitted |
| learning-reviewer (agent) | 58 | 4 | 4 | 3 | 3 | revise | S | per stop | Told to iterate "IDs from context" that the spawn prompt never passes |
| rule-reviewer (agent) | 54 | 4 | 4 | 4 | 3 | keep | S | per stop | "Archive" vs `delete` wording |
| duplicate-detector (agent) | 33 | 4 | 4 | 4 | 3 | keep | S | per stop | No report format |
| session-summarizer (agent) | 56 | 4 | 4 | 4 | 4 | keep | S | per stop | — |
| git-history-analyzer (agent) | 220 | 4 | 4 | 3 | 4 | retire from universal | S | never | Nothing spawns or references it; uses tools not in its `tools:` list |
| issue-intelligence-analyst (agent) | 225 | 4 | 4 | 2 | 4 | retire / merge → cas-github-issues | S | never | Nothing spawns it; contradicts itself on `upstream` vs `origin` |
| codex factory-supervisor (agent) | 42 | 2 | 4 | 2 | 2 | revise | S | yes (Codex) | Untiered spawn recipe; forbids the monitoring commands cas-supervisor requires |
| CLAUDE.md directive (init) | 15 | — | 3 | 3 | 3 | revise | S | yes (all projects) | Duplicates cas-task-tracking; ships a Petrastella Slack rule universally |

Distribution: **keep 8 · revise 30 · merge 2 · retire 2**. Median scores D=4, S=4, C=3, A=3 — descriptions are mostly fine, content accuracy and agentic sharpness are where the debt sits.

## Ranked fix list

### P0 — wrong or stale content that misleads an agent today

| # | Where | Defect | Evidence |
|---|---|---|---|
| 1 | `agents/task-verifier.md:307,314,321,328` | Verdict templates pass `files="…"`; the schema field is `files_reviewed` and the struct is not `deny_unknown_fields`, so the value is silently dropped on every verification | `types/verification.rs:33 pub files_reviewed: Option<String>` |
| 2 | `agents/task-verifier.md:208` | `git diff --name-status HEAD~10 \| rg -E '…'` — `-E` is ripgrep's `--encoding`; the test-first check errors every time | `printf 'A x\n' \| rg -E '^A'` → `error parsing flag -E … unknown encoding` |
| 3 | `cas-worker/references/close-gate.md:59`, `recovery.md:19`, `cas-worker.md:26`, `cas-github-issues/SKILL.md:55` | Teach the retired `pending_supervisor_review` status; close parks in `awaiting_merge` | `CHANGELOG.md:37-39`; migration `m248_tasks_retire_pending_supervisor_review.rs`; `close_ops.rs:1266-1267,1448` |
| 4 | `cas-supervisor/references/reference.md:66-74` (+ `close-gate.md:40`, `recovery.md:5`, `cas-supervisor-checklist.md:101`) | Teach `bypass_code_review=true` as the override flag; it is a schema-hidden one-release alias for `supervisor_override` | `types/task.rs:192-197` (`#[schemars(skip)] rename="bypass_code_review"`), `service/core.rs:663`, `task_claiming.rs:758` |
| 5 | `cas-supervisor.md:17`, `workflow.md:249`, `codex/agents/factory-supervisor.md:40` | "The documented critical escape hatch" for supervisor close is referenced three times and defined nowhere; `supervisor_override` appears in no supervisor document | `grep -rn "escape hatch"` → only these lines; `close_ops.rs:369-396` implements it |
| 6 | `cas-supervisor/references/workflow.md:39`, `codex/agents/factory-supervisor.md:27`, **and** `mcp/tools/service/factory_ops.rs:1749-1750` | Direct supervisors to `/epic-spec` and `/epic-breakdown`; no such skills exist in any tree — the live factory prompt says it too (Rust fix) | `ls cas-cli/src/builtins/skills \| grep epic` → 0 |
| 7 | `workflow.md:209-214` vs `:130-139` | Two contradictory merge procedures: `worktree_merge` is primary vs "cherry-pick to base branch, one per commit" (which also violates `epic-driving.md:5`) | quoted lines; `worktree_ops.rs:292-294,515` |
| 8 | `codex/agents/factory-supervisor.md:30,41` | `spawn_workers count=N` with no lane/model contradicts "Tier every spawn"; "never run `git log`, task list polling, worker status" contradicts `worker-recovery.md:61-63` and `workflow.md:175` | side-by-side quotes |
| 9 | `cas-task-tracking.md:31`, `cas-search.md:63`, `reference.md:5` | "Exact list — do not invent others" omits real actions: task `cancel request_changes reset proposal_*`; search `history retrieval_feedback retrieval_metrics skill_impact` | `service/mod.rs` dispatch arms (task 91-118, search 569-585) |
| 10 | `cas-memory-management/SKILL.md:146,100` | `mode=autofix` "reserved for Phase 2 … returns an error" — it is live with `Merged`/`Conflict` variants and `expected_updated_at` | `core/memory.rs:418,508`; `types/memory.rs:131,210-221` |
| 11 | `cas-memory-management/references/overlap-detection.md:68`, `body-templates.md:152` | `cas memory refresh` / `cas memory migrate` do not exist | `cas memory --help` → hygiene/share/unshare; `cas memory-migrate` is the legacy-store migration |
| 12 | `overlap-detection.md:15,169` | `--no-overlap-check` flag; real parameter is `bypass_overlap=true` | `grep -rn no-overlap-check cas-cli/src --include=*.rs` → 0; `types/memory.rs:113` |
| 13 | `cas-memory-management/SKILL.md:21` vs `:47`, `schema.yaml:8,36`, `overlap-detection.md:82,102-103` | Two contradictory type enums, and a markdown-file store model ("files live at ~/.claude/projects/…/memory/", "delete the stale file", "update MEMORY.md index") for what is a SQLite entry store | `types/memory.rs:62`; Cassy reads only `name/description/module/track/root_cause` from YAML inside `content` (`core/memory.rs:162-173`) |
| 14 | `cas-brainstorm/SKILL.md:5-8`, `cas-ideate/SKILL.md:5-8` | `disallowed-tools: Write, Edit` forbids the tool their own write-the-artifact phases require (`brainstorm:213`, `requirements-capture.md:140-146`, `post-ideation-workflow.md:80-97`) | `sync/skills.rs:111-114` emits the block; Claude Code honours it |
| 15 | `design-spec/SKILL.md:9,43` | "the design-review persona reads this file" — persona layer removed in v3.10.0 | `CHANGELOG.md:33-36`; `grep -rn design.review cas-cli/src --include=*.rs` → only dataviz include lines |
| 16 | `release-notes/SKILL.md:18-30` | cas-src transport policy (`pippenz@gmail.com` profile, `docs/SLACK_POSTING_RUNBOOK.md`, "Default Codex workers") baked into a builtin installed in every project; the runbook is not shipped | `builtins.rs:371-376,720`; `grep -rln SLACK_POSTING_RUNBOOK cas-cli/src` → skill bodies + marker test `builtins.rs:4154` only |
| 17 | `mcp-integration/SKILL.md:61-82,189-193` | Teaches only `claude mcp add/get/list/remove`; never mentions `cas mcp add` (drop-in), `cas mcp list` (connects + counts tools), `cas mcp import`, `.cas/proxy.toml`, or `mcp__cas__system proxy_add/remove/list/health` | `cas mcp add --help`; `service/mod.rs:875-881,1138-1181`; `runtime.rs:319-344` |
| 18 | `cli-routing/SKILL.md:37`, `references/routing.md:89-90`, `cas-image-generate/references/providers.md:3`, `asset-playbook.md:3` | Links of the form `../../../../../docs/…` and `…/.cas/artifacts/cas-1c67/…` resolve only from the source tree; broken once installed under `~/.claude/skills` (and the dossier does not exist even here) | `ls .cas/artifacts/cas-1c67/research/` → No such file |
| 19 | `session-learn/SKILL.md:78` | Claims the Stop hook embeds this body via `include_str!`; the runtime has its own divergent inline prompt | `hooks/handlers/handlers_session.rs:1648-1700`; `grep -rn include_str cas-cli/src/hooks \| grep -i session` → 0 |
| 20 | `verify-before-claim/SKILL.md:72` | "(see close-gate.md)" with no path; the file is `cas-worker/references/close-gate.md`, unreachable from this directory | `find cas-cli/src/builtins -name 'close-gate*'` |
| 21 | `agents/learning-reviewer.md:16` + `session_stop/mod.rs:229-233` | "For each learning ID from context" — the spawn prompt passes no IDs and `memory list` has no unreviewed filter; the subagent cannot enumerate its work | `types/memory.rs:8-50` |
| 22 | `fallow/SKILL.md:53-54` vs `:164-299` | Rules mandate `--format json --quiet 2>/dev/null \|\| true`; not one workflow example uses it, so copying an example hits the exit-1 cancellation the rule warns about | `grep -c '\|\| true' SKILL.md` → 1 |

Runtime residue of the removed review layer (Rust, not markdown, but it makes the stale skill *visible* today):

| # | Where | Defect | Evidence |
|---|---|---|---|
| R1 | `builtins.rs:2283-2331 prune_stale_cas_skill_dirs` | Deletes only `cas-*` dirs **without** `managed_by: cas`, so a *removed managed builtin* is never pruned — `cas-code-review` still exists in `~/.claude-daniel@petrastella.io/skills/`, `~/.claude-support@gabber.studio/skills/`, and the repo's `.claude/skills/`, and is listed (as "DEPRECATED") in every session's skill menu | `head -5 ~/.claude-daniel@petrastella.io/skills/cas-code-review/SKILL.md` → `managed_by: cas` |
| R2 | `close_ops.rs:13316` | Close rejection text still says "The full cas-code-review skill will be run by the supervisor" | quoted |
| R3 | `.claude/workflows/cas-code-review-prototype.js` (+ `-constants.js`, `merge-findings*.js`) | Orphaned workflow that loads `.claude/skills/cas-code-review/references/personas/`; `BUILTIN_WORKFLOWS` is now empty (`builtins.rs:468`) and nothing removes the old file | `ls .claude/workflows` |
| R4 | `builtins/reference-history.json:47` | Still ledgers the removed persona file `skills/cas-code-review/references/personas/fallow.md` | quoted |

### P1 — description / trigger / routing fixes

| # | Where | Fix |
|---|---|---|
| 1 | `cas-dataviz/SKILL.md:3` | Triggers are word-for-word Claude Code's bundled `dataviz` skill; scope the description to "static, self-contained SVG figure + data table for Cassy reports/issues/notes; bundled `dataviz` owns interactive charts" |
| 2 | `cas-image-generate/SKILL.md:3` | Lead with the trigger, not the provider; name the SVG-first route the body makes the default (`asset-playbook.md:24-26`) |
| 3 | `cli-routing/SKILL.md:3,23`, `routing.md:54,60`, `release-notes/SKILL.md:18-30` | Remove the operator e-mail from shipped text; express the Claude account gate as "an e-mail on the operator-configured allowlist" (a config key), and move the transport route to the project rubric |
| 4 | `cas-nuxt-playwright/SKILL.md:3-5` | Replace "Opt-in only: invoke ONLY when…" with a real trigger naming the stack (Nuxt 3/4 + Firebase auth + Quasar) and set `disable-model-invocation: true`; drop redundant `user-invocable: true` (`sync/skills.rs:518-520`) |
| 5 | `cas-tdd/SKILL.md:3`, `cas-wizard/SKILL.md:3` | Move the "Use when …" clause first; drop the identity sentence |
| 6 | `session-learn/SKILL.md:3` | Say what it produces (classified JSON drafts) and that it hands each draft to cas-memory-management |
| 7 | `codex/agents/factory-supervisor.md:3` | Role summary → trigger: "Use when running as the Codex factory supervisor: apply Codex constraints, then follow cas-supervisor" |
| 8 | `cas-supervisor-checklist.md:11` | Point at `cas-supervisor/references/preflight.md`, not a non-existent "Pre-flight section" |
| 9 | `cas-brainstorm/references/handoff.md:87`, `requirements-capture.md:76` | Hand off to cas-supervisor (epic creation), not `/plan` (does not exist) |
| 10 | `codemap/SKILL.md:187` | "Missing → PreToolUse blocks worker dispatch" is false: the gate fires only on `SignificantlyStale`, for supervisors, on `task create`/`spawn_workers` (`pre_tool.rs:352-356`) |
| 11 | `design-spec/SKILL.md:123-125,146` | No hook or CLI checks `DESIGN.md` (`grep DESIGN.md cas-cli/src --include=*.rs` → 0 outside builtins.rs); say "commit it so reviewers can diff it", do not promise a signal |
| 12 | `project-overview/SKILL.md:156-165` | Add the commit step; git history is the primary freshness signal (`project_overview.rs:525-527`) |
| 13 | `release-notes/SKILL.md:43,75`, `RUBRIC-template.md:21` | "Exactly one threaded reply" is a hard rule the home repo's own rubric overrides (`docs/RELEASE_SLACK_RUBRIC.md:31,151`); make reply-count a rubric default |
| 14 | `cas-viktor/SKILL.md:15-17` | Show the actual call: `mcp__cas__mcp_search code="server:viktor"` then `mcp__cas__mcp_execute code='{"server":"viktor","tool":"ask_viktor","args":{…}}'` (`ops_secondary.rs:1144-1163`) |
| 15 | `cas-search.md:17` | `doc_type` omits `artifact`; `scope` and `tags` filters never mentioned (`types/search.rs:19,25,31`) |
| 16 | `cas-memory-management/references/lifecycle-and-storage.md:22` | `update` cannot change title or validity (`types/updates.rs:5-23`) |
| 17 | `verify-before-claim/SKILL.md:87` | "Tasks marked additive-only" → `execution_note=additive-only` (`close_ops.rs:3321`) |
| 18 | `cas-writing-for-agents/SKILL.md` | Add the steps and completion criterion it demands of others; state the "Use when …" convention, the frontmatter fields, the three-mirror rule, and a line budget; link it from cas-worker for skill-edit tasks |
| 19 | `agents/task-verifier.md:25,116-138,362-390` | Replace the never-emitted "VERIFICATION JAIL" trigger with the real strings (`VERIFICATION DISPATCH INVALID`, `Verifier handoff rejected`); give `ast-grep` an `rg` fallback (not installed); document the stranded-branch and `epic_verification_owner` gates (`close_ops.rs:2116-2160,1966-1998`) |
| 20 | `agents/git-history-analyzer.md:26,199`, `issue-intelligence-analyst.md:206-207` | Tell subagents to use `AskUserQuestion` and MCP tools that their `tools:` list excludes; nothing spawns either agent — retire from the universal set or wire and fix |
| 21 | `workflow.md:273` | Final-gate log to `/tmp/…` contradicts the durable-proof contract (`cas-supervisor.md:25`; `[factory] artifacts_root`) |
| 22 | `docs_and_skill.rs:22` (CLAUDE.md directive) | The Slack release-notes rule is a Petrastella policy injected into every project on `cas init`; make it rubric-driven or project-configurable |

### P2 — formatting, length, duplication

- **Worker guidance over budget.** `worker_guidance()` is 9,956 B against `SESSION_START_BUDGET_BYTES = 9*1024` (`session_budget.rs:38`); it is protected content, so the degradable segments are dropped. Move "Structured execution state" (`cas-worker.md:31-46`) and "Context budgeting" (`:143-150`) to `details.md`; the "never block the pane / headroom % / facts not narration" block is stated three times (body `:86-88,104-117`, `discipline.md:20-104,229-375`, and verbatim in the spawn prompts `crates/cas-pty/src/pty.rs:20,41`).
- **Harness-enforced rules restated as prose.** AskUserQuestion-is-blocked appears 9× across cas-brainstorm/cas-ideate/intake.md while `pre_tool.rs:119-123` denies the call with that guidance; protected-branch commit, unscoped test run, and local-merge push denials (`pre_tool.rs:148,168,943-949`) are restated in `cas-worker.md:20,131` and `discipline.md:106-121`; `cas-supervisor.md:14-15` spends two hard-rule bullets on tools the harness already intercepts (`teams.rs:730`).
- **Supervisor tree duplication.** Lane table stated 5× and "Terra is suspended" 4× in `model-selection.md`; spawn cookbook byte-identical in `model-selection.md:139-156` and `workflow.md:74-91`; `code-review-queue.md` is a copy of `workflow.md:215-227` under a stale filename; `references/model-selection.md` and `planning.md` carry SKILL frontmatter although they are references; 44 lines of OpenCode token-plan detail (`model-selection.md:55-98`) in every supervisor's routing doc.
- **Doc-family boilerplate.** `codemap:99-137`, `project-overview:100-138`, `design-spec:86-121` share ~35 lines of keep-block / pointer-memory / commit procedure each → one `references/doc-hygiene.md`.
- **Tiny references behind pointers** (violating the yardstick's own rule at `:35`): `SKILL-MECHANICS.md` (13 lines), `DEEPENING.md` (10), `DESIGN-IT-TWICE.md` (7), `cas-tdd/mocking.md` (11), `tests.md` (9). Inline them.
- **Stance before procedure.** `cas-html-reports/SKILL.md:7-49` (first step at line 51), `mcp-integration` (ladder at `:87`), `fallow` (procedure at `:373`), `cas-github-issues:18-34`, `intake.md:5-7`, `preflight.md:10-57`.
- **Ticket-phase and machine narration** as a reliable stale marker: "Phase 1 / Phase 2" in `cas-memory-management:96-146` and `cas-search:28`; "v1 ships as advisory" in `verify-before-claim:89-97`; "Decision: in-process vs subprocess" in `session-learn:76-97`; "Verified on this machine" in `cas-codex-exec:13` and `routing.md:53` (already stale: says Claude Code 2.1.231, box has 2.1.257); "Tonight's successful operator pattern" in `reminders.md:92-94`; "observed 2026-08-11" in `cas-github-issues:141`; "Do not do external research in v1" in `cas-ideate:121`; the 10-line HTML changelog comment shipped on every `task-verifier` spawn (`:8-17`).
- **Downstream-project leakage** into borrowed skills: NestJS `*.service.ts` carve-outs in `cas-tdd:32`, `mocking.md:9-11`, `cas-codebase-design:32-34`; absolute operator path in `cas-dataviz/references/design-review.md:3`; "H7 acceptance-report precedent" in `cas-dataviz:45` (unresolvable).
- **worker-recovery.md:147-169** teaches a raw `UPDATE tasks SET status='closed'` for binaries predating `bba6fbf` — the only SQL write in the skill tree; `:117-123` documents context bands as absolute tokens while the code classifies percent-of-window (`factory_ops.rs:8314-8321`).
- **Contract drift in the guard itself.** `builtin_flavor_drift_test.rs:368` compares only `.md` files, which is why `cas-wizard/template.sh` (24 lines canonical, 18 in both twins — the example block is missing) drifted undetected.
- `builtins.rs:28-30` `TASK_TRACKING_GUIDE` / `MEMORY_GUIDE` / `SEARCH_GUIDE` are dead constants; the doc comment "Shared skills preloaded into factory sessions" is false (`builtins.rs:2898` asserts task-tracking loads on demand).

### P3 — nice to have

- `cas-worker.md:72` "Report Cassy defects to `Richards-LLC/cassy`" contradicts CLAUDE.md's in-repo rule and ignores `mcp__cas__system action=report_cas_bug`.
- `reference.md:39`, `workflow.md:54` use "awaiting review" as if it were a task status; only `awaiting_merge` exists (`cas-types task.rs:60-65`). `planning.md:160` says `design_notes` (a spec field); the task field is `design`.
- `rule-reviewer.md:22` says "Archive" but executes `rule action=delete` (no archive action exists). `duplicate-detector.md:31` "flag for manual review" names no channel.
- `cas-task-tracking.md:27` omits `note_type=question` (`notes.rs:48`); `cas-diagnosing-bugs` never names `mcp__cas__task action=notes note_type=discovery`; `cas-resolving-merge-conflicts:13` should say `note_type=decision`.
- `cas-codex-exec:16` pins `-m gpt-5.5` while the box default is `gpt-5.6-sol` — omit `-m`. Two different canonical `codex exec` recipes exist (`cas-codex-exec:16` vs `routing.md:20-24`).
- `cas-wizard/template.sh:4,11` — `set -e` + `confirm` returning 1 aborts unless wrapped in `if`; the example never shows the safe form.
- `fallow:15,316,365` "90" vs `:214` "91" plugins. `agents/*:10` hard-coded "Current year: 2026".
- `mcp-integration:80` should mention `cas mcp list --json` redacts secrets by default (`--show-secrets`, cas-9f07).
- Model pins: `task-verifier` is `model: sonnet` — the only gate between a worker's close and the merge is pinned weaker than the Fable 5.1 workers it judges; recommend `inherit` (or opus). `learning-reviewer` (haiku) promotes text into rules injected into every future session; recommend sonnet. Haiku is fine for duplicate-detector / rule-reviewer / session-summarizer.

## Cross-cutting themes

1. **Half-propagated removals.** v3.10.0 removed the review layer, m248 retired a status, and `bypass_code_review` became an alias — each is fixed in Rust and in *some* markdown. The mirror discipline (three flavours, byte-identical after prefix substitution) makes every stale string a three-file fix, and the drift guard then *pins* the stale text until all three change together. A grep-based "retired vocabulary" test (`pending_supervisor_review`, `bypass_code_review`, `persona`, `/epic-spec`, `cas-code-review`) would have caught all of it.
2. **"Exact list — do not invent others" lists that are themselves wrong.** Three skills carry hand-maintained action lists that lag the dispatch table. Generate them from `service/mod.rs` (or pin them with a test that parses the match arms) — the phrase "do not invent others" turns an omission into an active prohibition of a real action.
3. **Always-loaded text paying for harness-enforced rules.** The worker guide blows the SessionStart budget partly by restating what `pre_tool.rs` denies anyway. Fable 5.1 follows a hard denial from the harness at least as well as a paragraph asking nicely; the paragraph costs context on every turn.
4. **Operator-specific facts in universal builtins.** An e-mail address, a Slack runbook path, a research-dossier path, a NestJS carve-out, an absolute `/home/pippenz/...` path, "verified on this machine with Claude Code 2.1.231". These belong in project rubrics, config keys, or memories — not in files `cas init` installs for every user.
5. **Stack-specific skills shipped universally.** `fallow` (3,360 lines, JS/TS only) and `cas-nuxt-playwright` (Nuxt+Firebase+Quasar) are synced into every project including this pure-Rust one, in three flavours. A language-gated or opt-in tier would cut ~7 k synced lines per project.
6. **Descriptions are the strong point.** 27 of 35 skills already lead with "Use when …"; the misses (cas-dataviz collision with the bundled skill, cas-image-generate leading with the provider, cli-routing's e-mail, cas-nuxt-playwright's shouted opt-in, cas-tdd/cas-wizard identity-first) are all one-line fixes.
7. **Agentic effectiveness lags.** Completion criteria and stop conditions are present in the factory core (close-gate, cas-github-issues, cas-servers, epic-driving) and absent in most methodology skills (tdd, codebase-design, domain-modeling, writing-for-agents, html-reports). The yardstick skill itself has no steps.

## Per-skill detail

Each block: scores · description now → proposed · findings (P-level, file:line, evidence) · worst paragraph rewrite · overlap.

### cas-worker (`skills/cas-worker.md` + `cas-worker/references/`)

D=4 S=3 C=3 A=3 · revise · L · **always loaded** via `worker_guidance()`

Description now: "Use when acting as a factory worker on an assigned Cassy task, including progress reporting, blocker handling, delivery, and supervisor handoff." → keep.

- [P0] `close-gate.md:59`, `recovery.md:19`, `cas-worker.md:26` — retired `pending_supervisor_review`; real park state is `awaiting_merge` with lease released (`close_ops.rs:1266-1267,1448`).
- [P1] `close-gate.md:40`, `recovery.md:5` — `bypass_code_review=true` deprecated alias (`close_ops.rs:396 DEPRECATED_BYPASS_WARNING`; guard text `close_ops.rs:6554-6560` already says `supervisor_override` does not skip merge-state checks).
- [P2] body is 9,956 B vs 9,216 B budget (`builtins.rs:2554-2556`; `session_budget.rs:38`).
- [P2] `:86-88,104-117` + `discipline.md:20-104,229-375` + `crates/cas-pty/src/pty.rs:20,41` — the pane/headroom/checkpoint discipline stated three times.
- [P2] `:20,131`, `discipline.md:106-121` — restate PreToolUse denials (`pre_tool.rs:148,168,943-949`).
- [P3] `:72` bug-report route contradicts CLAUDE.md; `discipline.md:3-7` opens with an unrelated "Marked throwaway prototypes" section.

Worst paragraph — `close-gate.md:59`. Before: "…persisting an immutable delivery transaction, then releases your lease and moves the task to `pending_supervisor_review` awaiting a fresh verification verdict." After: "…persists an immutable delivery record, releases your lease, and parks the task in `awaiting_merge` for the supervisor's merge and verdict. Omit it for the ordinary close path. Rejection returns `DELIVERY RECEIPT REJECTED` and changes nothing."

Overlap: `discipline.md` Parts 1–3 duplicate the spawn prompts; `details.md:3-15` duplicates cas-search; `details.md:83` and `cas-task-tracking.md:31` both carry an action list.

### cas-supervisor (`skills/cas-supervisor.md` + `cas-supervisor/references/`)

D=5 S=4 C=3 A=3 · revise · M · **always loaded** via `supervisor_guidance()` (5,604 B of an 8,192 B ceiling, `builtins.rs:2508`)

Description: keep.

- [P0] `:17` "documented critical escape hatch in workflow.md" — undefined; `supervisor_override` (`close_ops.rs:369-396`) never named.
- [P2] `:23,33,37-41` tiered-spawn rule three times (~600 B). `:55` References list still names `code-review-queue`.
- [P3] `:14-15` AskUserQuestion / raw Agent already intercepted (`pre_tool.rs:119`, `teams.rs:730`). `:63` cites `project_session_start_truncation.md` as a repo doc; it is a memory. `:20-22` posture bullets without criteria.

Worst — `:17`. After: "Never close a worker's task. The only exception is `task action=close id=<id> supervisor_override=true reason="<why>"` (supervisor-only, reason required, logged); use it only when the worker is confirmed dead or the binary is stale, and note it on the task."

References (loaded on demand):

| File | Lines | Verdict | Key findings |
|---|---|---|---|
| `code-review-queue.md` | 24 | merge → workflow.md | Verbatim copy of `workflow.md:215-227`; first sentence defers to workflow.md |
| `epic-driving.md` | 12 | keep | Best-shaped file in the tree; `proof_scope_fix` undocumented in reference.md; `:12` is a maintainer instruction |
| `filing-cas-bugs.md` | 33 | keep | `cas config get/set issues.repo` verified; trim `docs/requests/` legacy prose |
| `intake.md` | 52 | revise | Posture before the gate; AskUserQuestion dup; overlaps checklist `:60` |
| `model-selection.md` | 215 | revise | Route table `:28-39` matches `crates/cas-factory/policy/lane-registry.toml` exactly; everything else restates it; `lane=<lane>` is the shortest correct recipe yet the doc pins cli/model/effort |
| `planning.md` | 162 | keep (trim) | `:160 design_notes` → `design`; empty H2 at `:144-146`; execution_note values verified |
| `preflight.md` | 57 | revise | Two commands then 47 lines of report internals; no fail branch |
| `reference.md` | 143 | revise | [P0] `:66-74` `bypass_code_review` on transfer; `:5` action list short; `:39` "awaiting-review" phantom |
| `reminders.md` | 94 | keep | All params/events verified (`factory_remind.rs:3-4,343,662`); delete `:92-94` narration |
| `worker-recovery.md` | 201 | revise | `:147-169` raw SQL; `:117-123` bands absolute vs percent; two dead-worker procedures (`:22-55` vs `:69-84`); `:86-105` "Injected but unwoken" is unique and good |
| `workflow.md` | 289 | revise | [P0] `:39` `/epic-spec`; [P0] `:209-214` vs `:130-139`; [P1] `:273` `/tmp` log; `:161-171` force/allow_trunk/cleanup table is correct — keep; delete `:236-243` |

### cas-supervisor-checklist (`skills/cas-supervisor-checklist.md`)

D=4 S=3 C=3 A=3 · revise · M · on demand (`builtins.rs:2852-2855`)

- [P1] `:11` "Pre-flight section in the cas-supervisor SKILL" does not exist → `cas-supervisor/references/preflight.md`.
- [P1] `:101` `bypass_code_review`.
- [P2] `:45` "cherry-pick into `develop` will abort later" — nothing cherry-picks or targets `develop` (`grep` → 0; `develop` only a default-branch candidate in `worktree/sweep.rs:463`).
- [P2] `:60-67,86-93` generic checkbox gates duplicating `intake.md`/`planning.md`; `:13-19,48-58` twelve lines of `cas --version` sed/awk commentary.
- [P3] `:97-101` epic close gate stated twice.

Worst — `:11`. After: "Confirm the running binary matches HEAD: `cas --version | sed -E 's/.*\(([0-9a-f]+)(-dirty)? .*/\1/'` vs `git rev-parse --short HEAD`. If they differ and `git log HEAD --not <hash> -- cas-cli/src/mcp cas-cli/src/hooks` is non-empty, stop and ask the operator to rebuild and reconnect MCP. Full procedure: cas-supervisor/references/preflight.md."

Target shape: a ≤40-line ordered session-start runbook; gates live in the supervisor references.

### cas-task-tracking (`skills/cas-task-tracking.md`)

D=4 S=4 C=2 A=3 · revise · S · **not** bundled at SessionStart despite the `TASK_TRACKING_GUIDE` constant

- [P0] `:31` omits `proposal_inbox/accept/reject/reconcile`, `cancel`, `request_changes`, `reset`; contradicts `details.md:83`; same in both mirrors.
- [P2] `:27` note types omit `question` (`notes.rs:48`).
- [P3] `:9` no `depth` / `execution_note` / priority alias mention (`types/task.rs:154-168`).

Worst — `:31`. After: "Valid actions (exact): create show update start close cancel reopen request_changes delete list ready blocked notes dep_add dep_remove dep_list claim release reset transfer available mine proposal_inbox proposal_accept proposal_reject proposal_reconcile. `request_changes`/`reset` are supervisor moves; `proposal_*` handle cloud-proposed tasks." Better: generate this line from the dispatch table.

### cas-memory-management (`skills/cas-memory-management/`)

D=4 S=2 C=2 A=2 · revise · L · `MEMORY_GUIDE` constant is dead (not injected)

Description: keep.

- [P0] `SKILL.md:146,100` autofix "reserved for Phase 2" — live (`core/memory.rs:418,508`; `types/memory.rs:210-221`); `expected_updated_at` (`types/memory.rs:131`) never mentioned.
- [P0] `overlap-detection.md:68`, `body-templates.md:152` — `cas memory refresh` / `cas memory migrate` do not exist.
- [P0] `overlap-detection.md:15,169` — `--no-overlap-check` → `bypass_overlap=true` (`types/memory.rs:113`).
- [P0] `SKILL.md:21` vs `:47`, `schema.yaml:8,36`, `overlap-detection.md:82,102-103` — two type enums; a markdown-file store model for a SQLite store. The skill never says "put the YAML frontmatter inside `content`", which is the only thing Cassy actually parses (`core/memory.rs:162-173`; `hybrid_search/filter_grammar.rs:20-27`).
- [P1] `lifecycle-and-storage.md:22` — `update` cannot change title/validity (`types/updates.rs:5-23`).
- [P2] `SKILL.md:98-147` two JSON dumps and Phase talk; missing live params `scope importance valid_until personal set_tier` (`types/memory.rs:32,80-99,146`).
- [P2] `overlap-detection.md:157-169` "Implementation notes (for the future Rust path)" — shipped (`crates/cas-core/src/memory/overlap.rs`).

Worst — `SKILL.md:143-146`. After: "`mode=autofix` merges into the high-overlap match atomically and returns `status=merged` (or `status=conflict` when `expected_updated_at` no longer matches — re-read and retry once). Default `interactive` returns `status=blocked`; then `action=update id=<existing_slug>` instead of re-inserting."

Overlap: owns `remember`; session-learn and cas-domain-modeling should point here.

### cas-search (`skills/cas-search.md`)

D=4 S=4 C=3 A=3 · revise · S · `SEARCH_GUIDE` constant is dead

- [P0] `:63` omits `retrieval_feedback retrieval_metrics skill_impact|impact_report history` (`agent_search_system/history.rs:28`).
- [P1] `:17` `doc_type` omits `artifact`; `scope`/`tags` absent (`types/search.rs:19,25,31`).
- [P2] `:28` "Phase 1 grammar limitation" — accurate (`filter_grammar.rs:54-60`) but phase-framed.
- [P3] `:52-59` all params verified.

Worst — `:61-63`. After: full action list + "Use `search` first for anything non-code; use the built-in Grep for known paths."

### session-learn (`skills/session-learn/SKILL.md`)

D=3 S=2 C=2 A=2 · revise · M

Description → "Use when asked to extract or save session learnings; classifies the transcript into concept/entity/correction/pattern/idea/decision/gap drafts, then hands each to cas-memory-management."

- [P0] `:78` false `include_str!` claim; Rust prompt at `handlers_session.rs:1648-1700` has diverged; skip floor `obs_count >= 5` (`stop_flow.rs:395-400`) and 500-byte guard (`handlers_session.rs:1620`) unstated.
- [P0] `:9,15,141` third-brain provenance and "Cassy does not yet have those skills" — no-op exposition.
- [P2] `:76-97` design notes (`memory.session_learn_auto` is real: `config/meta/seed/memory.rs:6`); `:21` "you do NOT write" is wrong for the user-invoked branch; `:44` restates the Rust-enforced overlap gate (`core/memory.rs:484-492`).

Worst — `:19-21`. After: "Output a JSON array of drafts (schema below). Hook-invoked: return the array only. User-invoked: show the array, then for each accepted draft call `mcp__cas__memory action=remember` per cas-memory-management and handle `status=blocked` there. Done when every draft is stored, corroborated, or explicitly dropped."

### verify-before-claim (`skills/verify-before-claim/SKILL.md`)

D=5 S=3 C=3 A=3 · revise · S

- [P0] `:72` "(see close-gate.md)" unreachable.
- [P1] `:87` `execution_note=additive-only` (`close_ops.rs:3321`).
- [P2] `:89-97` "Decision: Advisory vs Required-Paste (v1)" narration; `:9-17,99-103` motivational prose + mantra restate the four steps.

Worst — `:91-97`. After: delete, replacing with "Close-gate checks are mechanical; this proof note is what the verifier quotes. If you cannot name and run a proof, do not close — post a blocker note instead."

Overlap: `cas-worker.md:23` invokes both this and close-gate.md; either fold the proof step into close-gate.md as "Check 7" or shrink this to ~30 lines so non-factory sessions keep a model-invoked trigger.

### cas-writing-for-agents (`skills/cas-writing-for-agents/`)

D=5 S=3 C=4 A=2 · revise · S

- [P1] `:12-46` principles only; no steps, no completion criterion (contradicts `:39`). `SKILL-MECHANICS.md` (13 lines) behind a pointer violates `:35`.
- [P2] Missing house facts: frontmatter fields (`name description managed_by disable-model-invocation disallowed-tools`), the "Use when …" convention (not pinned anywhere: `grep -n '"Use when' builtins.rs` → 0), the three-mirror rule, a line budget.
- [P3] Nothing links to it (`grep -rln cas-writing-for-agents builtins/` → only its mirrors).

Worst — `:14`. After: "Steps: 1) description = `Use when <trigger>…`, one trigger per branch; 2) body = imperative steps, each with an observable done-state; 3) one statement per rule; no instruction the harness already enforces; 4) references only for a branch that earns it; 5) update the codex and grok mirrors. Done when the file is under ~80 lines and every line is live."

### cas-tdd · cas-diagnosing-bugs · cas-codebase-design · cas-domain-modeling

| Skill | Scores | Verdict | Findings |
|---|---|---|---|
| cas-tdd (39+20) | D3 S3 C4 A3 | revise S | `:3` identity before trigger → "Use when a task requires test-first work, red-green-refactor, seam selection for tests, or integration-test design."; `:32` + `mocking.md:9-11` NestJS leakage; `mocking.md`/`tests.md` restate `:24-32`; only Cassy rule (`:18,39` no zero-test success) duplicates `cas-worker.md:131`. Worst `:11` → "Record the red run and the green run (`scripts/run-scoped-tests.sh …`, test count, exit 0) with `mcp__cas__task action=notes note_type=progress`." |
| cas-diagnosing-bugs (62) | D5 S4 C4 A4 | keep | References no Cassy tool (`grep -o mcp__cas__` → 0); "task note" at `:42,62` should be `action=notes note_type=discovery`. Tight; enforces CLAUDE.md "don't assume — verify". |
| cas-codebase-design (57+17) | D4 S3 C4 A3 | revise S | Inline `DEEPENING.md`/`DESIGN-IT-TWICE.md`; `:32-34` NestJS; `:52` generic `mcp__cas__spec`. Worst `:48-53` → "Done when a task note lists: chosen seam, interface facts callers must learn, complexity hidden, deletion-test result, two rejected alternatives. If hard to reverse, also `mcp__cas__spec action=create`." |
| cas-domain-modeling (47) | D4 S3 C4 A3 | merge → cas-codebase-design | `:27-33` restates memory-management; `:37-41` restates lifecycle table; the unique 10 lines ("challenge the language") fit as a section of codebase-design. `:46` stray comma. |

### cas-resolving-merge-conflicts · cas-to-questionnaire · cas-wizard

All keep. `cas-resolving-merge-conflicts:13` → `action=notes note_type=decision`; `:9` provenance line is dead context. `cas-to-questionnaire` is correctly `disable-model-invocation: true` (human-facing description is right per SKILL-MECHANICS); `:14` "user-approved output location" undefined. `cas-wizard` description → "Use when a human must perform setup, secrets, dashboard, cutover, or migration steps; generates an interactive Bash wizard. Not for work the agent can do itself."; `template.sh:4,11` `set -e` + `confirm` returning 1 aborts unless in `if` — the example never shows the safe form; template drifted from mirrors (see drift section).

### cas-brainstorm (`skills/cas-brainstorm/`)

D=4 S=3 C=3 A=4 · revise · M

- [P0] `:5-8` `disallowed-tools: Write, Edit, NotebookEdit` vs `:213` "Write or update a requirements document" and `requirements-capture.md:140-146` `mkdir -p docs/brainstorms`.
- [P1] `handoff.md:87`, `requirements-capture.md:76` hand off to `/plan` (does not exist).
- [P2] AskUserQuestion-blocked sentence 5× (`:37,86,171`, `handoff.md:13,19`); harness enforces (`pre_tool.rs:119-123`). `:23-42` + `:232-242` rules stated three times.
- [P3] `:46` MIT attribution in always-loaded body; `:133-134` `task action=list status=closed` with no query.

Worst — `:32-35`. After: "Interaction rules: one question per message; use AskUserQuestion (falls back to plain text automatically in factory mode); ask what the user already thinks before offering ideas; broad before narrow."

### cas-ideate (`skills/cas-ideate/`)

D=4 S=4 C=3 A=4 · revise · S

- [P0] `:5-8` same `disallowed-tools` contradiction vs `post-ideation-workflow.md:80-97` (write `docs/ideation/…`) and the Quality Bar `:206`.
- [P2] `:121` "not in v1"; `:32,67` + `post-ideation-workflow.md:154` AskUserQuestion boilerplate; `:83,127` volume math twice.
- [P3] `:94` Haiku scan agent vs `:127` "do NOT tier down" reads as a contradiction — label the two dispatches.

Worst — `:13-19`. After: "Output: `docs/ideation/YYYY-MM-DD-<topic>-ideation.md`, a ranked survivor list with every rejection reasoned. Acting on a survivor goes through `cas-brainstorm`; never straight to planning or code."

### project-overview · codemap · design-spec (doc family)

| Skill | Scores | Findings |
|---|---|---|
| project-overview (189) | D4 S4 C4 A3 · revise S | [P1] `:156-165` no commit step though git is the primary signal (`project_overview.rs:520-527`); `cas project-overview clear` real (`cli/project_overview_cmd.rs:17-18`). `:144-146` knowledge build lacks bounded framing (default 90 s, `knowledge_cmd.rs:108`). `:129` memory title carries `.md` suffix (design-spec uses `_designmd`). `:46` lists `docs/plans/` which no skill produces. Worst `:156-165` → "Commit `docs/PRODUCT_OVERVIEW.md`, then run `cas project-overview clear`. Verify with `cas project-overview status` → `up to date`." |
| codemap (202) | D4 S4 C4 A4 · revise S | [P1] `:187` gate claim wrong (`pre_tool.rs:352-356` fires only on `SignificantlyStale`, supervisors, `task create`/`spawn_workers`; Missing → `severity=high` banner only, `codemap.rs:239`). `:144-153` ten lines of exit-status capture + 90 s deadline prose. `:163,176,198` git-sole-authority stated 3× (`codemap.rs:340-345`). Worst `:151` → "The build is bounded to 90 s and cleans up after itself; do not background or poll it. Non-zero exit: note the command and status, continue — the CODEMAP on disk is still the artifact." |
| design-spec (157) | D4 S4 C3 A3 · revise S | [P0] `:9` removed persona; [P1] `:43` "recurring design-review findings"; [P1] `:123-125,146` promises a drift signal no code emits. `:139-141` one-bullet section. Worst `:9` → "Front-end workers read this file instead of grepping components and theme files; point every UI task at it." Its token-source probe list (`:20-33`) is the unique, valuable part. |

Shared: `codemap:99-137`, `project-overview:100-138`, `design-spec:86-121` are ~35 lines of the same keep-block / pointer-memory / commit procedure → one `references/doc-hygiene.md`.

### release-notes (`skills/release-notes/`)

D=4 S=4 C=2 A=4 · revise · M

- [P0] `:18-30` cas-src transport policy in a universal builtin (see P0 #16).
- [P1] `:43,75`, `RUBRIC-template.md:21` reply-count hard rule contradicted by the home rubric (`docs/RELEASE_SLACK_RUBRIC.md:31,151`; `docs/release-notes/RUBRIC.md:6-7` says it wins) — so "project rubrics may add, never relax" (`:16,78`) is already violated by its own repo.
- [P2] `:48-54` + template `:37-46` + the CLAUDE.md snippet restate the same five rules (fourth copy in `RELEASE_SLACK_RUBRIC.md`).
- [P3] `:35` `git log <last-release>..HEAD` unresolvable for a staging merge; `gh pr view <n> --json commits` works.

Worst — `:20-23`. After: "Before posting, run the transport preflight named in the project rubric. If this session has no approved Slack transport, save the draft, hand path + channel + deploy target to whoever does, and report the duty blocked — never mark POSTED without returned timestamps."

### cas-github-issues (`skills/cas-github-issues/SKILL.md`)

D=4 S=4 C=4 A=5 · revise · S

- [P0] `:55` `pending_supervisor_review`. The substring-match warning itself is correct (`query.rs:50`).
- [P2] `:18-34` seventeen lines of preamble (banner and `cas config set issues.repo` are real: `session_budget.rs:444-450`, `gh_graphql.rs:62`).
- [P3] `:204-212` scheduled-task expiry is harness behaviour pinned only by a marker test (`builtins.rs:3881-3883`); `:141-143` date narration. All `gh` flags and MCP params verified.

Worst — `:52-56`. After: "Do not pass `status=open`: the filter is a substring match, so `in_progress`, `blocked`, and `awaiting_merge` tasks drop out and the sweep re-tasks live work. List everything and skip closed/cancelled yourself."

The merged-not-closed rule (`:170-172`) is the sharpest stop condition in the whole set — a model for the others.

### cas-html-reports · cas-dataviz · cas-image-generate

| Skill | Scores | Findings |
|---|---|---|
| cas-html-reports (124+644) | D4 S3 C5 A3 · revise M | `:7-49` stance before the first step at `:51`; `:86-107` restates `technical-contract.md` §1-7; `:118-119` lists `report-types.md` under "Worked examples"; `review-checklist.md` two-minute version last. Both example HTML files honour the contract (0 `http://`, `beforeprint` at eng:264 / fin:339). Worst `:11-15` → "Write `docs/<area>/YYYY-MM-DD-<topic>.md` first; render `<same>.html` from it (one file, no network, no JS-only content); commit both together. HTML never edits the markdown." |
| cas-dataviz (62+44) | D2 S4 C4 A4 · revise S | [P1] `:3` triggers identical to bundled `dataviz`. `:45` "H7 acceptance-report precedent" unresolvable; `design-review.md:3` absolute operator path; `:9,49,51` "inverts the bundled skill" 3×. Validator bundling claim verified (`builtins.rs:353,708,1086`). Description → "Use when a Cassy report, GitHub issue, or Markdown note needs a static, self-contained figure (inline SVG + data table, print-safe) or when a document is becoming text-dense. Claude Code's bundled `dataviz` skill owns interactive/library charts; this skill owns durable evidence artifacts." |
| cas-image-generate (72+546) | D3 S4 C4 A4 · revise S | [P1] `:3` provider-first; SVG-first route omitted. [P2] dossier links to a non-existent path. Script matches docs (`generate-image.sh:65-66,73,147`; `GEMINI_API_KEY`). `:70` "Imagen retired 2026-08-17" unverifiable; `providers.md:36-97` sixty lines of curl for four unwired providers. Description → "Use when a project needs a hero, background, logo, icon, OG card, illustration, or report artwork that must match its existing design system; routes flat/geometric assets to agent-authored SVG and photographic or painterly work to Google Nano Banana via `GEMINI_API_KEY`." |

### cas-servers · mcp-integration · cas-viktor · cli-routing · cas-codex-exec

| Skill | Scores | Findings |
|---|---|---|
| cas-servers (101) | D5 S4 C5 A5 · keep | Every param verified (`crates/cas-mcp/src/types/ops_secondary.rs:533-752`; `server_ops.rs:80-145,198`); cgroup claim verified (`ui/factory/cgroup.rs`). Only nit: "never background yourself" stated 3× (`:9,85`, rule 1). Worst `:20-25` → "Teardown kills the worker's whole cgroup, including `setsid`/detached children. Only `server_start shared=true` places a server outside that scope." |
| mcp-integration (215+216) | D4 S2 C2 A3 · revise L | [P0] `claude mcp` only; `cas mcp` / proxy absent (see P0 #17). `:108-139` duplicates `diagnosis.md:150-172`; `:13-15` field-notes voice; ladder at `:87`; `diagnosis.md:160` hedged env vars; `:80` omit `--show-secrets`. Viktor-specific scopes/run handles (`diagnosis.md:117,158`) belong in cas-viktor. Worst `:59-63` → "Register with `cas mcp add` (same flags as `claude mcp add`: `-s local\|project\|user`, `-t`, `-e`, `-H`). `local` is keyed by directory, so `.cas/worktrees/*` workers cannot see it; use `user` or `.cas/proxy.toml` for fleet access. Verify with `cas mcp list` and `mcp__cas__system proxy_health`." |
| cas-viktor (36+53) | D4 S4 C4 A4 · revise S | [P1] `:15-17` never shows the `code=` param / dispatch JSON. Allowlist (`crates/cas-mcp-proxy/src/config.rs:14-22`, nine tools) and cadence (`viktor_watch.rs:13-21`) verified; key procedure duplicated in `:31-36` and `gateway.md:3-11`. |
| cli-routing (47+104) | D2 S4 C3 A4 · revise M | [P1] operator e-mail; [P0] source-tree-only runbook links; `routing.md:53` stale version pin; release-note posting sequence in three places. Codex half duplicates cas-codex-exec with different flags. Description → "Use when a bounded, non-interactive task needs a one-shot `codex exec` or `claude -p` subprocess (capacity recovery, release-note posting). Codex first; Claude only after the account gate in references/routing.md passes." |
| cas-codex-exec (66) | D5 S5 C3 A4 · revise S | `-m gpt-5.5` pin drifting (box default `gpt-5.6-sol`); "verified on this machine" narration; conflicting recipe with `routing.md:20-24`. Worst `:13-17` → "`timeout 600 codex exec -s read-only -C "$PWD" -o /tmp/codex.out "<prompt>" < /dev/null` — read-only sandbox, config-default model. Add `-m <slug>` only when the supervisor names one." |

### fallow · cas-nuxt-playwright (stack-specific)

| Skill | Scores | Findings |
|---|---|---|
| fallow (382+2,978) | D4 S2 C3 A2 · revise + opt-in M | [P0] rules vs examples (`\|\| true`); procedure last (`:373-382`); `:100-151` MCP/Node sections irrelevant to a CLI agent; `builtins.rs:401-421,746-762,845` sync ~3 k lines ×3 into every project; `:39-49` three install paths, no detection (`which fallow \|\| npx fallow`); 90 vs 91 plugins. Worst `:373-382` → "Procedure: 1) `which fallow \|\| npx fallow`. 2) `fallow <cmd> --format json --quiet 2>/dev/null \|\| true`, filtered by issue type. 3) For `fix`: dry-run, show diff, then `--yes`. 4) Report counts + findings; suggest suppressions. Done when JSON parsed and summary delivered." |
| cas-nuxt-playwright (228+265) | D2 S4 C4 A3 · revise S | [P1] shouted opt-in → `disable-model-invocation: true`; [P1] body is Firebase+Quasar specific (`builtins.rs:390`) while the description says "Nuxt + Playwright"; `:5` `user-invocable: true` redundant; no completion criterion. Should point at cas-servers for the webServer. |

## Agent definitions

| Agent | Spawned by | Scores | Verdict | Findings |
|---|---|---|---|---|
| task-verifier (409) | close jail `close_ops.rs:2538`; identity `verification_tools.rs:131-135`; hook matcher `config_gen.rs:449` | D3 S2 C2 A3 | revise L | [P0] `files=` → `files_reviewed`; [P0] `rg -E`; [P1] `:25` "VERIFICATION JAIL" never emitted (`close_ops.rs:2551`, `verification_tools.rs:541`); [P1] `:116-138` `ast-grep` not installed; [P1] `:362-390` epic gates undocumented (`close_ops.rs:2116-2160,1966-1998`) while "all subtasks closed" (`:375`) is *not* enforced on close (only cancel, `:4456-4478`) so must stay; [P2] `:8-17` changelog comment on every spawn; `:98` `search action=search` hits memory not the diff; `:79,89` `HEAD~10` arbitrary vs task-attributed branch (`close_ops.rs:6167`); Phase 2 (`:229-299`) is a generic review rubric. `model: sonnet` → `inherit`. |
| learning-reviewer (58) | Stop hook `session_stop/mod.rs:227` | D4 S4 C3 A3 | revise S | [P0] no ID enumeration path; `mark_reviewed` rule 3×; all actions valid. Fix hook side too (pass IDs). haiku → sonnet. |
| rule-reviewer (54) | Stop hook `mod.rs:333`; `hooks_and_code.rs:481` | D4 S4 C4 A3 | keep | "Archive" vs `delete` (tombstone); conflict channel unnamed; actions and statuses verified (`cas-types rule.rs:64-71`). |
| duplicate-detector (33) | Stop hook `mod.rs:427`; `stop_flow.rs:655` | D4 S4 C4 A3 | keep | No report format although the hook asks for statistics (`mod.rs:429-432`). |
| session-summarizer (56) | Stop hook `mod.rs:500`; `stop_flow.rs:666` | D4 S4 C4 A4 | keep | Restates the hook prompt; tags match the re-trigger guard (`mod.rs:441-444`). |
| git-history-analyzer (220) | **nothing** (`grep -rn git-history-analyzer cas-cli/src` → builtins.rs only) | D4 S4 C3 A4 | retire from universal | `:26` AskUserQuestion and `:199` MCP call outside `tools: Read, Bash, Glob, Grep`; `:204-209` claims callers that do not reference it; never mentions `mcp__cas__search action=blame/history`. |
| issue-intelligence-analyst (225) | **nothing** | D4 S4 C2 A4 | retire or merge → cas-github-issues | `:23` "prefer upstream" vs `:205` "use origin" (upstream = `codingagentsystem/cas`, push disabled); MCP calls outside `tools:`; `:212-213` "invoked by cas-ideate" false (`grep -in issue cas-ideate/SKILL.md` → 0). |
| codex factory-supervisor (42) | Codex supervisor launch | D2 S4 C2 A2 | revise S | [P0] `:30` untiered spawn; [P0] `:41` forbids required monitoring; [P0] `:27` `/epic-spec`, `design_notes`; no Claude/Grok twin so every non-Codex line is drift by construction. Reduce to §Codex Constraints (`:9-13`) + "follow cas-supervisor". |

## The CLAUDE.md directive and the init-written `cas` skill

`cas init` writes two always-loaded artifacts into every project (`cas-cli/src/cli/init/docs_and_skill.rs`): the CLAUDE.md block (`:8-22`) and a project-level `cas` skill (`:150-216`). Together with `cas-task-tracking.md` that is **three** always-available statements of "use `mcp__cas__task`, not TodoWrite", with three different action subsets.

- [P1] `:22` — the Slack release-notes rule is a Petrastella policy shipped universally; this repo now carries it plus two rubrics.
- [P2] `:13` — the ToolSearch two-step is accurate for Claude Code (MCP tools are deferred) and pinned by tests (`:288-309`, incident cas-e7c8); keep it.
- [P2] `:14-18` — five action bullets duplicate `cas-task-tracking.md:12-24`; `:10` forbids `EnterPlanMode` "for task tracking" — a category error that deters legitimate planning.
- [P2] `CAS_SKILL` (`:150-216`) lists a `start=true` create param and `loop_start/loop_status/loop_cancel` guidance found in no other skill; it should be the pointer, not a fourth manual.

Proposed block: "Track work with `mcp__cas__task`, memory with `mcp__cas__memory`, context with `mcp__cas__search` (skills: cas-task-tracking, cas-memory-management, cas-search). First call per session: `ToolSearch(query="select:mcp__cas__task,mcp__cas__memory,mcp__cas__search")` — it only loads schemas; then call the tools directly and never re-run ToolSearch for a resolved tool. Release-note duties, if any, are defined by `docs/release-notes/RUBRIC.md`."

## Projection drift (Claude canonical → Codex → Grok)

**Maintenance model:** hand-maintained copies. `builtins.rs` embeds each flavour with its own `include_str!` (`BUILTIN_SKILLS` :111, `CODEX_BUILTIN_SKILLS` :472, `GROK_BUILTIN_SKILLS` :858); OpenCode alone is a process-local projection (`project_opencode_catalog`, `:1194-1220`, string-replacing the tool prefix). `scripts/gen-builtin-reference-history.sh` only ledgers SHA-256s of shipped reference files for the sync layer; it does not generate the twins. Equality is guarded by `cas-cli/tests/builtin_flavor_drift_test.rs`, which canonicalises the sanctioned spellings and compares section by section — for `.md` files only (`:368`).

**Measured drift** (`diff -rq` + per-file changed-line counts, then re-diff after applying the sanctioned substitution):

| Comparison | Files differing | Changed lines | After `mcp__cas__ → mcp__cs__` / `cas__` substitution |
|---|---|---|---|
| skills → codex | 41 of 87 | 432 | 1 file still differs: `cas-wizard/template.sh` |
| skills → grok | 42 of 87 | 454 | 3 files: `cas-wizard/template.sh`; `cas-supervisor.md` Heterogeneous-Teams heading (sanctioned, `CANON_HETERO`); `cas-viktor/SKILL.md` line-wrap only |
| agents → codex | 7 of 7 + `factory-supervisor.md` codex-only | 23/file (task-verifier) | 0 non-mechanical |
| agents → grok | 7 of 7 | — | 0 non-mechanical |

Characterisation: the projections are **99 % mechanical prefix substitution** and the guard works for markdown. The one real drift is `cas-wizard/template.sh` — canonical is 24 lines, both twins 18; the missing six lines are the `# Example:` block (`template.sh:19-24`) — invisible to the guard because it is not `.md`. Codex-only files: `agents/factory-supervisor.md` and `skills/cas-codex-supervisor-checklist.md` (sanctioned in `ALLOWED_FLAVOR_ONLY`), the former carrying three P0 contradictions with the skill it claims to follow.

Harness-appropriateness: the twins are correct for their prefixes (`mcp__cs__`, `cas__`, and `cas_` for OpenCode via `project_opencode_catalog`). Because the Codex/Grok spawn prompts (`crates/cas-pty/src/pty.rs:20,41`, 3.7 kB / 3.3 kB) already contain the pane/headroom/checkpoint discipline verbatim, Codex and Grok workers that also load `cas-worker` receive it twice.

## Overlaps, merges, retirements, gaps

**Merge**

- cas-domain-modeling → cas-codebase-design (keep the 10 unique lines as a section).
- `cas-supervisor/references/code-review-queue.md` → `workflow.md` (delete file).
- Codex recipe in `cli-routing/references/routing.md:20-24` → cas-codex-exec; cli-routing keeps the account gate and fallback policy only.
- Viktor scopes / run handles in `mcp-integration/references/diagnosis.md:117,158` → cas-viktor.
- codemap + project-overview + design-spec shared boilerplate → one `references/doc-hygiene.md`.
- verify-before-claim's four-step proof → `close-gate.md` "Check 7", *or* keep it at ~30 lines for non-factory sessions.
- `SKILL-MECHANICS.md`, `DEEPENING.md`, `DESIGN-IT-TWICE.md`, `cas-tdd/mocking.md`, `tests.md` → inline into their parents.

**Retire (from the universal set; keep available opt-in if wanted)**

- `git-history-analyzer`, `issue-intelligence-analyst`: never spawned, never referenced; tool lists contradict their bodies.
- `fallow`, `cas-nuxt-playwright`: stack-specific; sync only when the project matches (`package.json` / `nuxt.config.*`) or on explicit enable.

**Missing skills the docs imply**

- A **retired-vocabulary lint** for builtins (test, not skill): fails on `pending_supervisor_review`, `bypass_code_review`, `persona`, `/epic-spec`, `/plan`, `cas-code-review`, `Phase 1/2`, operator e-mails, `../../../../` links, and action names absent from the dispatch table.
- A **supervisor-override procedure** (currently the undefined "escape hatch") — belongs in `cas-supervisor/references/workflow.md`.
- `docs/brainstorms/2026-04-09-planning-pipeline-requirements.md:84` and `docs/ideation/2026-05-06-skill-map-index-pattern-research.md:63` mention `cas-doc-review`, `cas-playwright-debug`, `cas-seo-expert` — deferred or never built; no action beyond noting that `cas-nuxt-playwright` absorbed the Playwright one (`docs/requests/completed/FEATURE-nuxt-playwright-skill.md:83`).

## Proposed follow-up epic

**Epic: Builtin skills hygiene wave (post-v3.10.0)** — target `main`; every markdown task touches the Claude, Codex, and Grok copies and the marker tests in `builtins.rs`.

| # | Title | Scope | Pri |
|---|---|---|---|
| 1 | Purge retired review-layer residue from runtime and builtins | Invert `prune_stale_cas_skill_dirs` so removed *managed* builtins are deleted (`builtins.rs:2283-2331`) and prune `.claude/workflows/cas-code-review-*.js`; fix `close_ops.rs:13316` message; drop `reference-history.json:47`; delete `pending_supervisor_review` and `bypass_code_review` from all skill copies (P0 #3, #4); fix `design-spec:9,43`; add the retired-vocabulary test | P0 |
| 2 | task-verifier correctness | `files=` → `files_reviewed`; `rg -E` → `rg -e`/`--regexp`; replace "VERIFICATION JAIL" with real strings; `ast-grep` fallback; document stranded-branch + `epic_verification_owner` gates; drop changelog comment; `model: inherit`; ×3 mirrors | P0 |
| 3 | Supervisor tree: define `supervisor_override`, kill `/epic-spec`, reconcile merge procedure | `cas-supervisor.md:17`, `workflow.md:39,209-214,249,273`, `reference.md:5,39,66-74`, `factory_ops.rs:1749-1750`; delete `code-review-queue.md`; drop the sqlite recipe and fix context bands in `worker-recovery.md`; `design_notes` → `design` in `planning.md` | P0 |
| 4 | cas-memory-management rewrite against the live API | autofix/`expected_updated_at`, `bypass_overlap`, one `entry_type` enum, "frontmatter inside `content`", remove file-store model, remove `cas memory refresh/migrate`, list live params, move JSON dumps to references | P0 |
| 5 | Generated "valid actions" lists | Replace hand lists in `cas-task-tracking.md:31`, `cas-search.md:63`, `reference.md:5`, `details.md:83` with one source pinned by a test that parses `service/mod.rs` dispatch arms | P1 |
| 6 | Worker guidance under the 9 KB budget | Move structured-state + context-budgeting to `details.md`; dedupe `discipline.md` vs `pty.rs` spawn prompts; drop harness-enforced restatements; fix `cas-worker.md:72` bug route; add a size test like `test_supervisor_guidance_under_8kb` | P1 |
| 7 | Description rewrites (eight skills) | cas-dataviz, cas-image-generate, cli-routing, cas-tdd, cas-wizard, cas-nuxt-playwright (+`disable-model-invocation`), session-learn, codex factory-supervisor — exact text in this report | P1 |
| 8 | De-operator-ise shipped builtins | Remove `pippenz@gmail.com`, `SLACK_POSTING_RUNBOOK` links, dossier links, `/home/pippenz` path, NestJS carve-outs, "verified on this machine" pins; add a config key for the Claude account allowlist; move transport route to the project rubric | P1 |
| 9 | Fix `disallowed-tools` on cas-brainstorm / cas-ideate; dedupe AskUserQuestion boilerplate; replace `/plan` handoff | frontmatter + `handoff.md:87`, `requirements-capture.md:76` | P1 |
| 10 | mcp-integration around `cas mcp` and the proxy | Rewrite `:59-82,189-193`; collapse body vs `diagnosis.md`; move Viktor material to cas-viktor; show the `mcp_execute` call shape in cas-viktor | P1 |
| 11 | release-notes: procedure only, rubric-driven | Drop `:18-30` transport policy; make reply-count a rubric default; update marker test `builtins.rs:4154`; reconcile the CLAUDE.md directive line (`docs_and_skill.rs:22`) | P1 |
| 12 | session-learn ↔ Stop hook single source; learning-reviewer gets IDs | Either `include_str!` the skill body in `handlers_session.rs:1648-1700` or drop the claim; pass the unreviewed IDs in `session_stop/mod.rs:229`; `model: sonnet` | P1 |
| 13 | Codex `factory-supervisor.md` → constraints + pointer | Keep `:9-13`, lane-tiered spawn recipe, delete `:41` monitoring ban | P1 |
| 14 | Doc-family shared reference | `references/doc-hygiene.md` for codemap / project-overview / design-spec; add commit step to project-overview; fix `codemap:187`; remove design-spec drift-signal promise | P2 |
| 15 | Merge and inline small skills/references | cas-domain-modeling → cas-codebase-design; inline five ≤13-line references; retire git-history-analyzer + issue-intelligence-analyst from `BUILTIN_AGENTS`/`REQUIRED_FACTORY_AGENTS` (or wire and fix their tool lists) | P2 |
| 16 | Opt-in tier for stack-specific skills | fallow + cas-nuxt-playwright synced only on stack detection or explicit enable; fix fallow rules-vs-examples and front-load its procedure | P2 |
| 17 | Upgrade the yardstick | cas-writing-for-agents gets steps, completion criterion, frontmatter fields, "Use when" convention, three-mirror rule, line budget; absorb SKILL-MECHANICS; link from cas-worker | P2 |
| 18 | Drift guard covers non-markdown | Extend `builtin_flavor_drift_test.rs:368` to `.sh/.js/.yaml`; restore `template.sh` twins | P2 |
| 19 | Front-load procedures | cas-html-reports, cas-github-issues, intake.md, preflight.md, cas-supervisor-checklist (≤40 lines), model-selection dedupe | P2 |
| 20 | Remove dead guide constants | `TASK_TRACKING_GUIDE`/`MEMORY_GUIDE`/`SEARCH_GUIDE` and the false "preloaded" doc comment (`builtins.rs:22-31`) | P3 |

## Provenance and search manifest

Repository: `/home/pippenz/Petrastella/cas-src` worktree `kind-lion-55`, HEAD `0489d956` (v3.10.1). Tools: `cas 3.10.1 (0489d95 2026-09-02)`, `rg`, `git`, `claude 2.1.257`, `codex-cli 0.152.0`. Every subagent finding above was spot-verified by the author against the cited file:line before inclusion; the P0 items #1, #2, #10, #14, #21 and R1 were re-executed directly.

Commands run (hits in parentheses; 0-hit greps are the evidence that a referenced thing no longer exists):

- `diff -rq cas-cli/src/builtins/skills cas-cli/src/builtins/{codex,grok}/skills` (41 / 42 files) and the per-file changed-line count loop (432 / 454 lines); re-diff after `sed 's/mcp__cas__/mcp__cs__/g'` and `…/cas__/g` (1 / 3 residual files)
- `grep -rhoE "mcp__cas__[a-z_]+ action=[a-z_]+" skills agents | sort -u` (60 pairs; all present in `service/mod.rs` dispatch)
- `grep -rn "pending_supervisor_review\|bypass_code_review\|multi-persona\|persona" skills agents` (12 hits) · `grep -rn "pending_supervisor_review\|bypass_code_review" cas-cli/src --include=*.rs` (25, all legacy-alias/migration)
- `grep -rn "cas-code-review" cas-cli/src --include=*.rs` (10; `close_ops.rs:13316` is user-visible) · `ls ~/.claude*/skills/cas-code-review` (2 installs) · `head -5 …/cas-code-review/SKILL.md` (`managed_by: cas`)
- `grep -rn "WORKER_GUIDE\|SUPERVISOR_GUIDE\|TASK_TRACKING_GUIDE\|MEMORY_GUIDE\|SEARCH_GUIDE\|CHECKLIST_GUIDE" cas-cli/src` (only `builtins.rs`; `supervisor_guidance()`/`worker_guidance()` called from `hooks_traits.rs:53-57,102-106`, `session_budget.rs:369`)
- `awk` body-size of `cas-worker.md` minus frontmatter and the structured-state section (9,956 B) vs `SESSION_START_BUDGET_BYTES` (`session_budget.rs:38`)
- `grep -n "epic-spec\|epic-breakdown" factory_ops.rs` (2) · `ls skills | grep -i "plan\|epic"` (0)
- `grep -rn '\.\./\.\./\.\./\.\./' skills agents` (5) · `grep -rln pippenz@gmail.com skills agents` (3) · `grep -rn "escape hatch" skills` (2 relevant) · `grep -rn "awaiting.review" skills` (2) · `grep -rn disallowed-tools skills` (3) · `grep -rc "AskUserQuestion is blocked" skills` (9)
- `grep -n "files_reviewed" types/verification.rs` (1) · `grep -n 'files="' agents/task-verifier.md` (4) · `printf 'A x\n' | rg -E '^A'` (error) · `grep -n '"autofix"' core/memory.rs` (2) · `sed -n 227,234p session_stop/mod.rs` (no IDs) · `sed -n 105,125p sync/skills.rs` (emits `disallowed-tools`)
- `cas --help`; `cas {factory,memory,knowledge,viktor,codemap,project-overview,known-repos,config,mcp,worktree,history} --help`; `cas skill`, `cas task`, `cas tools` (unrecognized)
- `sed -n 80,137p builtin_flavor_drift_test.rs` (exemptions) · `grep -n '\.md\|extension' builtin_flavor_drift_test.rs` (`:368` `.md` only)
- `grep -rn "Phase 1\b\|Phase 2\b" skills agents` (18) · `grep -rn "3.10.0" CHANGELOG.md` (`:31`)

Companion HTML: `docs/analysis/2026-09-02-builtin-skills-review.html` (generated from this file). Raw reviewer batch notes: `/home/pippenz/.cas/artifacts/cas-4bd6/raw/`.

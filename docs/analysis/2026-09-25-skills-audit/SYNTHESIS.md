# Skills & prompts audit 2026-09: synthesis and proposed fix plan

EPIC cas-1660 · task cas-77915 · 2026-09-25 · calm-owl-92. This document reports findings only: no code
or skill was edited and no cargo command was run.

Inputs, all in this directory:

| Lane | Task | Report | Scope |
|---|---|---|---|
| L1 | cas-63c5 | `L1-rubric.md` + `L1-findings.md` | formats, house standard, harness parity |
| L2 | cas-3e02 | `L2-findings.md` | factory core skills, built-in agents |
| L3 | cas-988a | `L3-findings.md` | runtime prompts in Rust, MCP tool descriptions |
| L4 | cas-a4d8 | `L4-findings.md` | design and reporting skills |
| L5 | cas-9233 | `L5-findings.md` | engineering, release, workflow skills |
| L6 | cas-ea56 | pending | installed instruction files |

All lanes worked from the same code baseline, `4836e56f7` (v3.31.0). The epic tip adds only audit
documents on top of that.

How rows are referenced:

- `L1#n`: row n of the L1 ranked-findings table.
- `L2 P0-nn` / `P1-nn` / `P2-nn`: row numbers as printed in L2.
- `L3 P0.n` / `P1.n` / `P2.n`: the n-th row of that severity in L3, counting only rows of that
  severity. The P3 provenance row that sits inside the L3 P2 table is skipped.
- `L4 Fn`: finding n in L4.
- `L5 P0-n` / `P1-n`: row numbers as printed in L5.

## 1. Verdict (one screen)

The five lanes reported **46 P0s and about 120 P1s**. After deduplication they come to **25 P0 and 34
P1 master rows** (§2). **Every P0 was re-checked against the current code:**

- 45 are confirmed.
- 1 is partly confirmed (L2 P0-21, where the fix scope is wider than reported).

Taken sentence by sentence, the shipped skills are well written. The failures are in the machinery
around them.

1. **Agents are given calls that fail as written. This is the largest P0 class.**
   - Runtime envelopes:
     - Claude workers are told to use Codex-spelled `mcp__cs__` tools.
     - Suggested `message` calls leave out the `summary` parameter the server requires.
     - Workers in `local_merge` mode are told to push.
     - Verifier guidance omits `dispatch_id`.
   - Skill examples:
     - `task create` examples leave out the required `risk`.
     - task-verifier sends `files_reviewed=`, which the server silently drops.
     - The worker's proof skill tells workers to run cargo, which the harness denies.
     - The dead-worker runbook shuts down the whole fleet.
     - `mcp_execute`, Viktor and fallow examples use the wrong call shape.
     - The session-learn prompt breaks its own Stop-hook parser.
   - Each of these costs a failed call and a recovery turn, and it recurs.
2. **Updates never reach installed copies.** Scripts and examples freeze at first install. Retired agents
   and removed references are never pruned. A shipped exemplar leaks operator e-mails and cost figures to
   every project. Because of the freeze, deleting the exemplar at the source will not remove it from
   installs.
3. **Always-loaded text exceeds harness limits that the code itself documents.**
   - The assembled SessionStart is 11.8 KB for workers and 13.2 KB for supervisors, above Claude Code's
     10,000-character hook cap. Past the cap the model sees only a ~2 KB preview.
   - The worker skill body has 11 B of headroom under its test cap.
   - The `coordination` tool description is cut at 2,048 characters.
   - About 11 KB of the MCP schema is rmcp boilerplate.
4. **The per-harness spelling model does not work in practice.** Grok and OpenCode load `.claude/skills`.
   AGENTS.md uses the Codex spelling together with Claude's ToolSearch syntax. Codex ignores `.md` agent
   files. Meanwhile three spellings are kept in sync through 414 embedded files and a 1,588-line drift test.
5. **Rules drift because they are copied by hand, and project-specific text ships everywhere.**
   - Hand-copied rules:
     - three worker contracts
     - four verification-timeout texts
     - three form-choice tables
     - six copies of the release-note content policy
     - lane matrices written out about eight times
   - Text that is specific to cas-src ships in every project's builtins: release-train steps, cargo
     triage, Richards-LLC links, and the rule "fix Cassy bugs here".

**What I need from the operator.**

- **Approve Wave A now:** WP1–WP4 plus the deprecated-name sweep, which is the M17 part of WP5 (§4).
  None of these changes the architecture. Together they:
  - fix the runtime-envelope P0s;
  - bring SessionStart back under the hook cap;
  - make later skill fixes reach installed copies.
- **Decide D1–D12 (§5)** before Waves C and D start.
- **Expected savings once Waves A and B land:**
  - Worker SessionStart shrinks by about 2.6 KB and actually reaches the model.
  - Worker MCP schemas shrink by about 4.5k tokens per session.
  - Each verification spawn saves about 3.2k tokens.
  - Each release cut saves about 5.7k tokens.
  - The recurring failed-call recovery turns go away.

## 2. Master findings table (deduplicated)

Each row takes the highest severity any lane gave it. Every lane P0 and P1 is listed by ID either in a
master row (column "Lane refs") or in the duplicate list (§2.4).

### 2.1 P0: misleads an agent today

✔ means the row was re-checked against the current code, which is identical to `4836e56f7`:

- M01–M09 were checked by me.
- The L2 and L5 rows were checked by a read-only sub-audit at `ca73bb591`; see §2.5.

| ID | Finding | Lane refs | Strongest evidence | ✔ |
|---|---|---|---|---|
| M01 | Claude workers get Codex `mcp__cs__` tool names in every assignment, stall nudge and reply footer. The prefix comes from the session-wide `worker_cli`, not from the recipient. | L3 P0.1 | `ui/factory/director/prompts.rs:1357-1393,744`; caller `app/mod.rs:1362` | ✔ |
| M02 | Suggested `coordination action=message` calls omit the required `summary`. Affects runtime templates and skill examples. | L3 P0.2, L2 P0-05 | `agent_search_system/message.rs:637-645`; `prompts.rs:744,1388`; `pty.rs:20,41,315,1404`; `worker-recovery.md:137` | ✔ |
| M03 | Text tells `local_merge` workers to push, and PreToolUse denies the push. Affects close remediation, the worker body and launch contracts. | L3 P0.3, L2 P0-10 | `close_ops.rs:10918,11786`; `cas-worker.md:37-39`; `pre_tool.rs:210-229` | ✔ |
| M04 | Direct-verdict guidance omits the required `dispatch_id`. | L3 P0.4 | `close_ops.rs:6381,6731`; `verification_tools.rs:337-345` | ✔ |
| M05 | Skill files that are neither `SKILL.md` nor `references/*` freeze at first install. Removed files are never pruned (for example `code-review-queue.md` is still installed). | L1#1 (L2, L4, L5 cross-refs) | `builtins.rs:2318-2339,2534-2549`; installed `cas-wizard/template.sh` ≠ source | ✔ |
| M06 | The cas-html-reports before/after exemplar is a real operator report: 3 e-mail identities, `~/.codex-support@…` paths, $ figures, task ids. It ships ×3 flavours, a regression of the 09-02 fix. | L4 F1 | `…/before-after/rubric-review-{before,after}.html` | ✔ |
| M07 | cas-release-report never mentions `cas release report`. 22 of 29 reports skip its brief/QA steps. | L4 F2 | `grep -c 'release report' SKILL.md` = 0; `cas release report --help` | ✔ |
| M08 | Public-surface QA gates need `visual-qa.mjs` / `terminal-qa.mjs`, which ship only in cas-src. The gate refuses `unavailable` without an override, so downstream web closes are refused. | L4 F5 (P1), L5 P0-6 | `qa_evidence.rs:475-483`; `find builtins -name 'visual-qa*'` = 0 | ✔ |
| M09 | The session-learn Stop prompt breaks its own parser: "omit the rest" drafts fail `serde` and drop the whole batch. It also ships maintainer sections and tool steps to a single-turn, tool-less call. | L5 P0-11, L3 P2.19, L5 P1-23, P1-24 | `hooks/handlers.rs:207-227`; `handlers_session.rs:1678,1758-1766,1782` | ✔ |
| M10 | task-verifier verdict templates pass `files_reviewed=`, which the tool silently drops. The wrong marker is pinned by a test. Regression of 09-02 P0 #1. | L2 P0-01 | `ops_secondary.rs:354-395` (`files`); pin `builtins.rs:6274` | ✔ |
| M11 | Proof and test guidance tells factory workers to run cargo, which the harness denies. Affects verify-before-claim, cas-tdd, the platform-proof gate and worker recovery triage. | L2 P0-02, L2 P1-36, L2 P2-81, L5 P1-31, L3 P2.18 | `verify-before-claim/SKILL.md:27-28,72-73`; `pre_tool.rs:30-69`; `close_ops.rs:1768-1800` | ✔ |
| M12 | `task create` examples omit the required `risk`: task-tracking, github-issues, brainstorm, ideate, and the planning field map. | L2 P0-06, L2 P1-42, L5 P0-7, P0-8, P0-9 | `mcp/tools/types/task.rs:124-140` | ✔ |
| M13 | Supervisor runbooks give wrong commands. <br>• `shutdown_workers count=0` kills everyone. <br>• `release` on a lease the supervisor does not own. <br>• Closing workers' tasks. <br>• `list status=open` as the "all closed" check. <br>• `scope=code`. <br>• A `sync_all_workers` force claim. <br>• Rebuild or restart `cas serve`. <br>• The valid-action list omits real actions. <br>• The stranded-branch override is called "unwaivable". <br>• Per-merge test reruns. | L2 P0-03, 04, 11, 12, 13, 14, 15, 17, 19, 22 | `factory_ops.rs:2778-2785,7628-7644`; `ops_task_leases.rs:174`; `search.rs:148-159`; `close_ops.rs:5529-5545` | ✔ |
| M14 | Spawn guidance contradicts the lane registry. <br>• Explicit `cli`/`model`/`effort` bypasses lane fallbacks. <br>• "docs-only → light" conflicts with taste; `model-selection.md:13` must change too. <br>• `config_dir` is described as Claude-only, but Codex uses `CODEX_HOME`. | L2 P0-16, 20, 21 (+ L3 `config_dir` schema text) | `ops_secondary.rs:746-751,1137`; `factory_ops.rs:53-65,2323-2407` | ✔ (P0-21 partial) |
| M15 | Worker references teach the wrong delivery state or rule. <br>• `completion_receipt` → "awaiting_merge" (actually `AwaitingVerification`, and only after merge). <br>• "Report headroom every note" contradicts the <20% rule. | L2 P0-09, 25 | `close_ops.rs:4166-4183` | ✔ |
| M16 | Maintenance agents cannot do their job. <br>• learning-reviewer's skill create omits `invocation`, and the defaults would publish globally. <br>• rule-reviewer's "promote" is one vote. | L2 P0-07, 08 | `service/core.rs:951-968`; `rules.rs:37-41,233-256` | ✔ |
| M17 | Deprecated names that expire next release: `issues.components.mecha_cassy` (both always-loaded bodies, CLAUDE.md block, pinned test) and `cas integrate mecha-cassy`. | L2 P0-18, L5 P0-2, L5 P1-25 | `config/access/mod.rs:8-13`; `cli/integrate/mod.rs:106-116` | ✔ |
| M18 | Memory entry-type "enum" leaves out the live `handoff` type. Unknown values silently become `learning`. | L2 P0-23 | `memory.rs:453-458,513` | ✔ |
| M19 | Cassy-bug routing in the always-loaded worker body. <br>• `report_cas_bug` files into the downstream project's tracker. <br>• cas-src-only "fix them here" text is injected everywhere. | L2 P0-24 | `agent_search_system/system.rs:263-277` | ✔ |
| M20 | fallow's `2>/dev/null \|\| true` hides every exit code. A missing binary looks like a clean pass. | L5 P0-1 | `fallow/SKILL.md:21-24,68,193`; live rc 0 on a nonexistent binary | ✔ |
| M21 | The release-notes RUBRIC template example uses Markdown `**`, which mecha-cassy lint refuses. | L5 P0-4 | `RUBRIC-template.md:88-95` vs `mecha-cassy/SKILL.md:36-37` | ✔ |
| M22 | The cas-qa-craft trigger only fires on `demo_statement`, but the gate also fires on user-facing paths and journeys. Agents following the skill get refused closes. Same text in `cas-worker.md:24`. | L5 P0-5 | `qa_pass.rs:100-108`; `qa_evidence_gate.rs:84-89` | ✔ |
| M23 | Wrong MCP call shapes. <br>• `mcp_execute server= tool= args=` (the only param is `code`). <br>• Viktor `question` (should be `message`). <br>• The schema text itself calls `code` "TypeScript". | L5 P0-3, P0-12 (+ L5 cross-lane 2) | `ops_secondary.rs:1256-1272`; live `ask_viktor` schema | ✔ |
| M24 | Instruction contradiction inside a skill: cas-brainstorm says "ONE question at a time" and also "ask the full frontier". | L5 P0-10 | `cas-brainstorm/SKILL.md:32` vs `:41` | ✔ |
| M25 | mcp-integration never mentions the fail-closed proxy allowlist (the test call is denied after every add). It also applies Claude `-s local` semantics to `cas mcp`. | L5 P0-13, P0-14 | `cas-mcp-proxy/src/config.rs:53-59,421-425`; `cli/mcp_cmd.rs:54,118-124` | ✔ |

### 2.2 P1: routing, format, budget, architecture

| ID | Finding | Lane refs | Theme |
|---|---|---|---|
| M30 | SessionStart exceeds the 10K hook cap for every factory role. Knowledge and Handoff are protected by omission. The worker body has **11 B** of headroom under its 8,000 B cap. The supervisor is 117 B over the ≤6,450 B operator target. | L3 P1.1, L2 P1-26, P1-28 | T3 |
| M31 | Claude workers on custom config dirs probably get no SessionStart (the fallback is supervisor-only). | L3 P1.3 | T3 |
| M32 | The `coordination` description is cut at 2,048 characters. The lost tail contradicts `force`. | L3 P1.4 (+ L2 P0-14 same rule) | T3 |
| M33 | The Knowledge index shows the alphabetically-first 11/148 pages regardless of task. | L3 P1.2 | T3 |
| M34 | Always-loaded text restates rules the harness already enforces. <br>• `USAGE_REMINDER` `<IMPORTANT>`. <br>• Supervisor hard rules for SendMessage, AskUserQuestion and worktree agents. <br>• Worker no-build, workspace and push prose. | L3 P1.8, L2 P1-30, P1-31 | T3/T7 |
| M35 | Worker and supervisor rules arrive twice at launch. <br>• Claude worker contract ≈1.8 KB duplicates the body. <br>• Claude supervisor contract re-invokes the injected skill plus checklist plus codebase-design (≈4.9k tok, 1.6k duplicate). | L2 P1-32, P1-33 (L3 P2.6) | T4 |
| M36 | Prefix bugs outside M01. <br>• Role-mismatch banner hardcodes `mcp__cs__`. <br>• Stop-hook jobs run the Codex bodies. <br>• Claude and Grok copies of 4 maintenance agents are listed but never spawned, and a Claude fallback would see `mcp__cs__`. | L3 P1.5, L2 P1-58 | T1 |
| M37 | Links in both always-loaded bodies are source-tree-relative and break once installed. | L2 P1-29 | T2 |
| M38 | Close-gate dead ends. <br>• WORKTREE MERGE JAIL requires a non-existent `worktree-merger` agent. <br>• MERGE REALITY contradicts the epic merge model. | L3 P1.6, P1.7 | T5 |
| M39 | Grok resolves all CAS skills from `.claude/skills` (Claude spelling). | L1#2 | T1 |
| M40 | OpenCode loads Claude-spelled skills. The `cas_` projection is never written. | L1#3 | T1 |
| M41 | Codex agents are TOML, so installed `.md` agents are inert. `factory-supervisor.md` has no consumer and stale content. | L1#4, L2 P1-48 | T1/T2 |
| M42 | Retired managed agents are never pruned from installs. | L1#5 | T2 |
| M43 | `disallowed-tools` is documented as a guard. It is turn-scoped and Claude-only. | L1#6 | T5 |
| M44 | AGENTS.md is generated in Codex spelling with Claude ToolSearch syntax and caps. Grok loads it twice. | L1#7 | T1/T7 |
| M45 | Opt-in skills are model-invocable in Codex and OpenCode. `disable-model-invocation` is ignored and `agents/openai.yaml` is never generated. | L1#10 (P2), L5 P1-10 | T1 |
| M46 | Supervisor references are stale or contradictory. <br>• Three dead-worker procedures. <br>• is-wedged table lacks approval-hang and uses the wrong bands. <br>• "verify in TUI" / "ask the supervisor". <br>• Untiered, non-isolated spawn examples. <br>• `max` effort list. <br>• Suspended terra listed. <br>• Unfiltered `remind_event`. <br>• cas-src SHA check in checklist step 0. <br>• "tests pass" per task. <br>• Orphaned intake.md/planning.md. <br>• Phantom mecha-cassy fallback. | L2 P1-34, 35, 37, 38, 39, 40, 41, 43, 44, 45, 47 | T4/T5 |
| M47 | task-verifier structure. <br>• Close-path section addresses the closer. <br>• `HEAD~10` base. <br>• Self-contradicting reject policy. <br>• No `tools:` restriction. <br>• 27.7 KB per spawn (P2-60). | L2 P1-49, 50, 59 (+ P2-60) | T4 |
| M48 | Stop-hook jobs run with identity stripped. The summarizer resolves the wrong caller and the duplicate detector ignores its job IDs. | L2 P1-51 | T5 |
| M49 | task-tracking, search and memory skills drift from the schema. <br>• `status=blocked` vs `action=blocked`. <br>• Missing `platform_proof`, `spec`/`artifact` doc types. <br>• Cap handling. | L2 P1-52, 53, 54 | T5 |
| M50 | Worker references. <br>• The clean-tree receipt applies to every close but is linked only for deep ones. <br>• A `vercel env pull` line contradicts the credential strip. | L2 P1-55, 56 | T5 |
| M51 | cas-src-only content ships in universal builtins. <br>• Surface checklist in the worker body. <br>• Release prebuild, refresh script, `nextest -p cas`, Richards-LLC links. <br>• Release-train receipt rules. <br>• Universal staging/main Slack duty. <br>• mecha-cassy "Cassy vX.Y.Z" content policy. | L2 P1-27, P1-57, L5 P1-3, P1-4, P1-5 | T6 |
| M52 | Release trio. <br>• cut-release reads a 31.9 KB failure log per cut (−5.7k). <br>• release-notes collides with Grok `/release-notes`. <br>• mecha-cassy promises diary posts but hard-codes one reply. | L5 P1-1, P1-2 (= L1#15), P1-6 | T4 |
| M53 | QA and Playwright skills. <br>• Telemetry step out of order. <br>• `cli attach` without `-s`. <br>• False q-btn claim. <br>• Frontend-engineering routes to the uninvocable nuxt skill. <br>• 1.59 vs 1.62 floor. | L5 P1-7, 8, 9, 11, 12 | T5 |
| M54 | fallow content is stale. <br>• 22 of 38 MCP tools listed. <br>• Node section. <br>• Plugin count. <br>• Exit codes 0–2 (3.x has 0–13). <br>• Vendored ≈2.57 references (−20.5k per read). <br>• Deprecated `setup-hooks` collides with `.claude/`. | L5 P1-13, 14, 15, 16, 17, 18 | T5 |
| M55 | Ideation pipeline. <br>• brainstorm and supervisor intake both create epics (duplicates). <br>• "Light lane" offered as an Agent option. | L5 P1-19, 20 | T4 |
| M56 | codemap / project-overview. <br>• "One build = one model call" is false with a stale ledger. <br>• Pointer update without a find step. | L5 P1-21, 22 | T5 |
| M57 | Tooling skills. <br>• codex-exec does not close stdin. <br>• Dangerous bypass flag where `-s workspace-write` suffices. <br>• cli-routing and codex-exec both fire. <br>• Viktor retries without `idempotency_key`. <br>• Phantom precedent. <br>• `open_url` opens twice. | L5 P1-26, 27, 28, 29, 30, 32 | T4/T5 |
| M58 | Release report triggers two skills with different heroes and layouts. | L4 F3 | T4 |
| M59 | Three contradictory form-choice sources (pie, KPI cards). | L4 F4 | T4 |
| M60 | Two token vocabularies with no mapping. design-spec diverges from the public DESIGN.md spec and its linter. | L4 F6, F7 | T4 |
| M61 | `generate-image.sh` cannot request aspect or size. | L4 F8 | T5 |
| M62 | Descriptions over budget: cli-craft 514, ui-craft 428. 5 skills are over 250. | L4 F9, L1#8 | T3 |
| M63 | Reference hygiene. <br>• References carry skill frontmatter. <br>• Orphans. | L2 P1-46 (L1#16 P3) | T2 |

### 2.3 P2 / P3 (grouped; full detail is in the lane reports)

| ID | Group | Lane refs | Δ tokens (est.) |
|---|---|---|---|
| M70 | MCP schema. <br>• Boilerplate (16.4% of `tools/list`). <br>• Supervisor-only params in worker tools. <br>• Action-list drift, and `action` is not an enum. <br>• Duplicated request structs. <br>• Stale values. | L3 P2.1–P2.4, L3 P3, L2 fix-order note (`mark_reviewed`, `request_changes`, `reset`) | −2.9k schema; −2–3k per worker session with the split |
| M71 | Ambient recall. <br>• Keys on envelope words. <br>• Re-injects the task the agent just read. <br>• Provenance noise. | L3 P2.5 (+P3) | −400…−800 per turn |
| M72 | Worker contract copies ×3, missing `remind_message`, `agent-authored` label, and other envelope polish. | L3 P2.6–P2.10, L3 P3 | −790 per spawn |
| M73 | Close-gate text helpers (MERGE REQUIRED, VERIFICATION REQUIRED, timeouts, receipt recovery, authority denials, impossible remedies). | L3 P2.11–P2.17 | −1k+ per rejection cycle |
| M74 | Factory-core dedupe. <br>• Lane matrix ×8. <br>• Maintainer text in the supervisor body. <br>• OpenCode detail. <br>• Incident narration. <br>• Receipt explanations ×3. <br>• task-verifier 27.7 KB. <br>• Emphasis in agents. | L2 P2-60…86 | −3.2k per verification; −3.5k supervisor refs; −2k worker refs |
| M75 | Design and report skills. <br>• 22.6 KB tokens JSON per render. <br>• Restated contract. <br>• 221 KB exemplars. <br>• Pasted PDF programs. <br>• Unwired providers. <br>• No TOCs. <br>• dataviz conflict rule. | L4 F10–F17, F19 | −4.9k per render |
| M76 | Workflow-skill dedupe. <br>• Release content policy ×6. <br>• nuxt-playwright duplicates. <br>• brainstorm/ideate. <br>• cli-routing. <br>• AskUserQuestion boilerplate. | L5 P2 (Appendices A–E), L5 token table | −1.5k per announcement, −1.3k, −1.4k, −0.5k |
| M77 | House standard `cas-writing-for-agents` lags 2026 guidance. <br>• No per-harness frontmatter matrix. <br>• No description budget. <br>• No model-era wording rules. <br>• "≤80 lines" is contradicted by 20 of 41 skills. | L1#12, L1 §2 | +250 per invoke |
| M78 | Top-level `managed_by: cas` is not portable; move it to `metadata.managed_by`. | L1#9, L4 F20, L5 scope note | ≈0 |
| M79 | Dead `/cas-start`, `/cas-context`, `/cas-end` prohibitions. | L1#11, L3 P3 | −25 |
| M80 | Operator names and ticket ids in shipped text. | L4 F18, L3 P3, L2 P2-80 | small |
| M81 | Polish, gathered from all lanes. | L1#14–17, L2 P3, L3 P3, L4 F21–F28, L5 P3 | small |

### 2.4 Duplicates merged

| Reported as | Merged into |
|---|---|
| Supervisor cross-lane P0 "non-SKILL.md files never refresh" | M05 |
| Supervisor cross-lane P0 "Grok in cas-src resolves `.claude/skills`" | M39 |
| L1#8 (descriptions > 250) | M62 |
| L1#15 / L5 P1-2 (release-notes vs Grok) | M52 |
| L1#16 (reference frontmatter, P3) | M63 (L2 P1-46) |
| L2 P0-05 (message without summary) | M02 |
| L2 P0-10 (worker body push) | M03 |
| L2 P0-14 (`sync_all_workers` force) | M13, same fact as M32 |
| L2 P1-48 (factory-supervisor.md) | M41 |
| L3 P2.6 (worker rules twice) | M35 |
| L3 P2.18 (platform proof before worker deferral) | M11 |
| L3 P2.19 (session-learn prompt) | M09 |
| L4 F5 (QA scripts unshipped) | M08 (raised to P0 by L5 P0-6) |
| L5 P1-10 (Codex `disable-model-invocation`) | M45 |
| L5 P1-23, P1-24 (session-learn) | M09 |
| L5 P1-25 (`mecha_cassy` key) | M17 |
| L5 P1-31 (cas-tdd cargo) | M11 |

### 2.5 P0 spot-verification

**Result: 45 of 46 lane P0s CONFIRMED, 1 PARTIAL.** The 39 L2 and L5 P0s were checked by a read-only
sub-audit against the tree at `ca73bb591`.

| Row | Verdict | Note |
|---|---|---|
| L2 P0-21 | PARTIAL | `model-selection.md:186` "docs-only → light" does conflict with `:16,180`. But `:13` itself says light covers "bounded chores, docs", so the fix must change `:13` too. The `depth` conflation is real. |
| L2 P0-04 | CONFIRMED, caveat | If the dead worker's lease has already expired, `task_claiming.rs:412-455` recovers it and `release` succeeds. The advice is still wrong for a live lease; `reset` is the right verb. |
| L2 P0-08 | CONFIRMED, caveat | Retrieval evidence (`rules.rs:240`) is a second promotion path the lane did not mention. The reviewer's single `helpful` vote still cannot promote on its own. |
| L5 P0-4 | CONFIRMED, caveat | The conflict sits in the template example `:88-95`; `:33-45` is prose about the rubric itself. |
| L2 P0-01…25 (other 22), L5 P0-1…14 (other 13) | CONFIRMED | Each checked at the cited file:line plus the cited code. For example: `ops_secondary.rs:354-395` has `files` and no `deny_unknown_fields`; `factory_ops.rs:2778-2785` treats `limit==0` as all workers; `types/task.rs:131-138` rejects a create without `risk`; `hooks/handlers.rs:207-227` has no `serde(default)`. |
| L1#1, L3 P0.1–P0.4, L4 F1, F2 | CONFIRMED | Checked directly for this synthesis. Examples: `builtins.rs:2318-2339,2534-2549`; installed `template.sh` ≠ source; 7+7 identity hits; `grep -c 'release report'` = 0; the four L3 literals re-grepped. |

## 3. Themes (root causes that span lanes)

**T1: Tool-prefix model.** Harness prefixes are baked into text when it is written or rendered, and the
prefix is taken from the wrong source:

- the session-wide harness instead of the recipient's (M01);
- a literal written after the remap pass (M36);
- the harness that wrote a directory instead of the harness that reads it (M39, M40, M44, M45);
- Codex job bodies reused for Claude fallbacks (M36).

The three-spelling catalog guarantees parity between source files, not between what each harness
actually receives. → decision D1.

**T2: Install lifecycle only ever adds.** Sync overwrites only files whose frontmatter it can match
(M05). The prune step removes neither agents (M42), non-`cas-` builtins, nor removed references. The
links in the source tree do not resolve in the installed layout (M37). No test compares an installed copy
with the catalog. The consequences cascade: the operator-data purge (M06), every script fix (M08, M20,
M57, M61) and every reference fix stay stale in existing installs until this is fixed. **WP4 gates the
install effect of WP5, WP7, WP9 and WP10.**

**T3: No one owns the always-loaded budget as a whole.** Individual components are budgeted; the
assembled payloads are not:

- SessionStart (M30, M31, M33);
- MCP descriptions (M32, M70);
- restated enforced rules (M34);
- duplicate launch contracts (M35);
- descriptions (M62).

The caps that matter live in the harnesses and fail silently: 10,000 characters for hook output, 2,048
for a tool description, 8,000 for the Codex listing fallback.

**T4: Rules are maintained as hand copies.** Examples:

- worker contracts ×3 plus the body (M35, M72);
- close-gate texts (M73);
- request structs (M70);
- lane matrix ×8 (M74);
- form tables ×3 (M59);
- token vocabularies ×2 (M60);
- release content policy ×6 (M76);
- dead-worker procedures ×3 (M46).

Parity tests check markers, not meaning. The fix pattern: one owning file, or one renderer, per rule.

**T5: Prose is not tied to what the code accepts.** This is the source of most P0s. Suggested calls:

- omit required params (M02, M04, M10, M12, M16, M23);
- name denied or wrong actions (M03, M11, M13);
- name non-existent agents, commands or scripts (M08, M38, M79);
- describe guards that are not guards (M43);
- name superseded CLIs (M07, M17, M54).

Fix patterns:

- Add one test that parses every `mcp__cas__<tool> action=<x> k=v` in shipped builtins and runtime
  templates and validates it against the dispatch table and the required fields.
- Route prohibitions through the hook that enforces them.

**T6: Operator-specific and cas-src-specific content ships to every project.** This covers M06, M19, M51
and M80. The 09-02 de-operator-ise item has regressed. Two things are needed:

- a lint over shipped builtins: e-mails, `/home/<user>`, `~/.codex-*`, `cas-[0-9a-f]{4}`, `Richards-LLC`,
  `nextest -p cas`;
- a home for cas-src-only guidance → decision D10.

**T7: Wording lags current model guidance.** Emphasis in always-loaded text (M34, M44), verification
scaffolding (L2 P2-70), and conflicting rules that stall GPT-6 (M24, M59, M46). This is handled by the
house-standard rewrite (M77) plus targeted edits.

## 4. Proposed fix plan (work packages)

Risk classes:

- **R-build:** a Rust change. Needs a supervisor assembly build plus the full test run.
- **R-pins:** edits text that tests pin: `cas-worker.md`, `cas-supervisor.md`, agents, tool or session
  text.
- **R-docs:** markdown only. Changes under `cas-cli/src/builtins` are still subject to the drift and
  description tests; adding or removing a file changes `builtins.rs` registrations and makes it R-build.

**P8** is the pin set from the 3.17.3 lesson:

1. `issue_intake_directive_test`
2. `factory_codex_skill_guardrails`
3. `session_start_issue_triage_test`
4. `builtin_doc_hygiene_test`
5. `builtin_flavor_drift_test`
6. `builtin_skill_description_test`
7. `--lib builtins`
8. `--lib cli::factory::parity`

**P-L2** is L2's full pin table (L2 §SessionStart measurement): P8 plus

- `agent_definition_contract_test`
- `verify_before_claim_skill_test`
- `mcp_action_surface_test`
- the `session_budget` unit tests
- the `builtins.rs` pins at `:3587–7499`

Run the whole set in one pass with no fail-fast.

| WP | Scope (master IDs) | Main files | Risk | Tests affected / to add | Est. savings | Depends on |
|---|---|---|---|---|---|---|
| **WP1** Envelope and remediation correctness | M01, M02 (runtime), M03 (runtime), M04, M36 banner, M38, M72 remind/push | `director/prompts.rs`, `app/mod.rs`, `cas-pty/src/pty.rs`, `close_ops.rs`, `handlers_session.rs`, `pre_tool.rs`, `ops_secondary.rs` | R-build, R-pins (pty contract markers, `factory_codex_skill_guardrails`, `--lib cli::factory::parity`) | Add: every suggested `action=message` literal carries `summary=`. Add: per-recipient prefix test (Claude worker in a Codex-default session). | ≥1 failed call + recovery (~300–800 tok) per assignment or rejection | — |
| **WP2** Always-loaded budget | M30, M33, M34, M35 (body side), M19, M51 (body parts), L2 P2-62/73 | `session_budget.rs`, `cas-core/.../build_start.rs`, `…/context/mod.rs`, `handlers_session.rs`, `cas-worker.md`, `cas-supervisor.md` | R-build, R-pins (P-L2 in full) | Add: assembled payload ≤ 9,216 B per role on a realistic fixture. Retarget the surface-checklist pin. | Worker −2.6 KB (assembled) and −1.6 KB body; supervisor −4 KB assembled, −0.9 KB body | — (**first**: every WP7 body fix needs its headroom) |
| **WP3** MCP schema diet and text | M32, M70 (no split), M23 schema text, M10 `files` alias, M14 `config_dir` text, M18 enum | `service/mod.rs`, `crates/cas-mcp/src/types*.rs`, `list_tools` post-process | R-build; pin `mcp_action_surface_test` | Add: every tool description ≤ 2,048; `action` enum = dispatch table; no `nullable`/`default:null` | −2.9k tok schema; −1.6k for the 4 always-selected tools | — (D2 extends it) |
| **WP4** Install sync and prune | M05, M42, M37 (+ link-resolution test), removed-reference prune, ledger glob, doctor install parity | `builtins.rs` (owner check, prune fns), `gen-builtin-reference-history.sh`, `reference-history.json`, doctor | R-build | `--lib builtins` sync tests. Add: installed-vs-catalog parity test and link-resolution test. | −120 always; makes every later fix reach installs | — |
| **WP5** Deprecated names and operator data | M17 (expires next release), M06, M80, T6 lint | skills ×3 flavours, CLAUDE.md block (`docs_and_skill.rs`), `issue_intake_directive_test`, exemplar deletions | R-build (registrations, pinned test) + R-docs | `issue_intake_directive_test`, drift test. Add: operator-data lint. | −42.9k on-demand; −170 KB ×3 binary | WP4, for installed copies |
| **WP6** Close-gate helpers | M73, M38 follow-through | `close_ops.rs`, `pre_tool.rs`, `supervisor_push.rs`, `neon_sql_guard.rs` | R-build | Close-ops message pins | −1k+ per rejection cycle | WP1 |
| **WP7** Factory-core skill accuracy | M02/M03 (skill side), M11, M12 (task-tracking, planning), M13, M14, M15, M22 (`cas-worker.md:24`), M46, M49, M50, M63, M74 | `cas-supervisor/**`, `cas-worker/**`, `cas-task-tracking.md`, `cas-search.md`, `cas-memory-management/**`, `verify-before-claim`, `cas-tdd` | R-pins (P-L2), R-docs | As P-L2; update the wrong pins (e.g. `builtins.rs:7495-7499` self-dispatch ×5) | −3.5k supervisor refs, −2k worker refs | **WP2** (headroom) |
| **WP8** Verifier and Stop-hook jobs | M10, M16, M47, M48, M09, M36 (Stop jobs), L2 P2-60/76 | `agents/*.md` ×3, `stop_flow.rs`, `hooks/handlers.rs` (`SessionLearnDraft` `serde(default)`), `handlers_session.rs`, `rules.rs` (optional `promote`) | R-build, R-pins (`builtins.rs:6270-6278`, `agent_definition_contract_test`) | Fix the `files_reviewed` pin; add a session-learn parse test | −3.2k per verification spawn; −1.1k per Stop | D9, D11, D12 |
| **WP9** Workflow skills accuracy | M12 (github-issues, brainstorm, ideate), M20, M21, M22 (qa-craft), M23 (text), M24, M25, M52–M57, M76 | fallow, release trio, qa-craft, nuxt/playwright, brainstorm/ideate, codemap/overview, mcp-integration, cli-routing, codex-exec, viktor, wizard | R-docs (plus R-build if scripts are added or removed) | drift, description, `cas_*_skill_test`, failure-log pin `builtins.rs:8315` | −5.7k per cut, −4.9k fallow, −20.5k per reference read, −1.5k per announcement | WP4 (scripts reach installs); D7, D10 |
| **WP10** Design and report skills | M07, M08, M58–M62, M75 | L4 skill set, ship `visual-qa.mjs`/`terminal-qa.mjs`, `tokens.css` | R-docs + R-build (new shipped files) | drift, description, image/drawing skill tests, token parity test | −4.9k per render; −75 always | WP4; D5, D7, D8 |
| **WP11** House standard and portable frontmatter | M43, M62, M77, M78, M79, M45 (Codex yaml) | `cas-writing-for-agents`, all frontmatter, `builtin_skill_description_test.rs` | R-docs + R-pins (P8) | Extend the ≤250 description cap to all skills; `is_managed_by_cas` accepts `metadata.managed_by` | −170 always ×harnesses | D1 (tool naming wording) |
| **WP12** Harness projection | M39, M40, M41, M44, L1#13 | sync paths, `cli/sync/agents_md.rs`, `docs_and_skill.rs`, Codex agents | R-build, large (drift-test redesign if D1 = neutral) | `builtin_flavor_drift_test`, `factory_parity_test`, AGENTS.md sync tests | −450 always (Grok), −60 per harness | **D1, D3, D6** |
| **WP13** Startup single source and coordination split | M31, M35 (contract side), M72 renderer, optional `coordination`/`factory` split | `pty.rs`, `app/mod.rs`, `cas-worker.md`, `ops_secondary.rs`, `service/mod.rs`, every skill naming supervisor actions | R-build, R-pins (P-L2) | Contract markers; add "SessionStart fired" telemetry | −790 per spawn; −2–3k per worker session if split | **D2, D4**; WP2 |

Cross-cutting test to add early (WP1 or WP3), for T5: a call-shape lint that extracts every
`mcp__cas__<tool> action=<a> key=value` from `cas-cli/src/builtins/**` and from runtime template strings.
It checks each against the dispatch arms and the request structs' required and known fields. That
single test would have caught M02, M04, M10, M12, M13 (`scope=code`, release) and M23.

**Suggested order.**

| Wave | Packages | When |
|---|---|---|
| A (parallel) | WP1, WP2, WP3, WP4, and the M17 part of WP5 | Now. Deprecated names expire next release. |
| B | WP5 remainder, WP6, WP7, WP8 | After WP4 and WP2 |
| C | WP9, WP10, WP11 | After D5/D7/D8/D10 |
| D | WP12, WP13 | After D1–D4, D6 |

Build and pin rules:

- One assembly build per wave.
- WP2, WP7, WP8 and WP11 all touch P-L2-pinned text. Land them in one wave only if P-L2 runs in a single
  no-fail-fast pass (memories "skill-text-pins" and "no-gate-loops").
- Every always-loaded body edit must first cut at least the bytes it adds.

## 5. Decide first (operator)

| # | Decision | Options | Recommendation | Gates |
|---|---|---|---|---|
| D1 | Tool naming in shipped text | (a) keep three spellings and fix delivery per recipient/harness; (b) **prefix-neutral catalog**: bare tool names in skills, prefix stated once in role guidance | (b). The spellings already fail to reach Grok and OpenCode, and Stop jobs already cross flavours. It retires ~276 embedded twins and most of the drift test. | WP11 wording, WP12, WP8 Stop bodies |
| D2 | Split `coordination` | (a) keep one tool, trimmed; (b) worker-facing `coordination` plus supervisor `factory` tool | (b). Workers stop loading ~8 KB of spawn/worktree/db params, and tool annotations become honest. The skill-text edits are mechanical. | WP13, WP3 extension |
| D3 | AGENTS.md projection | (a) Codex spelling (today); (b) prefix-neutral, no ToolSearch line, plain imperatives; (c) per-harness files | (b) | WP12 |
| D4 | Canonical carrier for worker startup rules | SessionStart body vs launch brief vs on-demand skill | Decide after WP2 telemetry answers M31. If SessionStart is unreliable, make the brief canonical and cut SessionStart to identity plus inbox. | WP13 |
| D5 | `cas-release-report` vs `cas release report` | CLI-first skill vs declare CLI reports exempt from brief/QA | CLI-first. Practice already follows the CLI. | WP10 |
| D6 | Codex agents | Emit TOML agents vs stop installing `.md` for Codex | Stop installing, unless Codex supervisors should delegate natively | WP12 |
| D7 | QA gates downstream | Ship `visual-qa.mjs`/`terminal-qa.mjs` in skills vs gate accepts `unavailable` outside cas-src | Ship them (+81 KB ×3). Otherwise the gate refuses every downstream web close (M08). | WP9, WP10 |
| D8 | Token schema | Adopt public DESIGN.md spec keys plus a Petrastella role map vs keep the house schema | Adopt the spec keys with a `maps:` note | WP10 |
| D9 | Verifier policy for NOT-EXERCISED rows | approve / reject / supervisor call | Operator call (L2 P1-50) | WP8 |
| D10 | Home for cas-src-only guidance | repo `CLAUDE.md`/`docs/`, or a cas-src-only overlay skill synced only into this repo | Repo docs plus one pointer. Keep universal builtins project-neutral. | WP2, WP7, WP9 |
| D11 | Rule promotion | document vote semantics vs add a Rust `promote` action | Add `promote`, so the reviewer stops inflating its own metric | WP8 |
| D12 | Stop-hook agent bodies | per-harness subagent files vs one job body with a prefix remap at prompt build | One job body (follows D1) | WP8, WP12 |

## 6. Lanes pending

L6 (cas-ea56, installed instruction files) is still in progress. Its P0 and P1 rows will be added to §2
under the same deduplication rules.

# L2 audit — factory core skills and built-in agent prompts

**Date:** 2026-09-25 · **Reviewed at:** `4836e56f7` (v3.31.0, installed `cas` 3.31.0 matches) ·
**Task:** cas-3e02 (EPIC cas-1660) · **Rubric:** L1 v1 (`~/.cas/artifacts/cas-63c5/rubric.md`) ·
**Status:** findings only; no skill, agent or Rust file was edited and no cargo was run.

## Verdict

The factory core is mechanically well-mirrored (0 non-mechanical drift across Claude/Codex/Grok
in all 43 in-scope files) and most 2026-09-02 P0s are fixed, but **25 P0s mislead an agent
today**. The worst five: the task-verifier now sends a parameter the tool silently drops (a
regression caused by the prior review's own fix); the worker's proof skill tells workers to run
the cargo commands the harness denies; the supervisor's dead-worker runbook shuts down the whole
fleet and uses a lease verb that cannot work; the documented task-create example is rejected by
the server; and the two maintenance agents that promote learnings cannot perform their promote
step. Structurally, **the worker body has 11 B of headroom** under its 8,000 B test cap, so
every fix below needs a cut first; and both always-loaded bodies ship cas-src-only text and
links that break once installed.

Numbers to carry away:

| Measure | Value |
|---|---|
| Supervisor SessionStart body (`supervisor_guidance()`) | **6,567 B ≈ 1,641 tok**; 2,649 B left of 9,216 B vs 2,400 B floor → **249 B headroom** |
| Supervisor + Codex worker note (≈801 B) | 7,368 B; remainder 1,848 B vs 1,400 B test floor |
| Worker SessionStart body (`worker_guidance()`) | **7,989 B ≈ 1,997 tok**; **11 B** under the 8,000 B component cap; 1,227 B under 9,216 B |
| Claude worker launch contract (`claude_worker_contract`) | 3,976 B, ~1.8 KB of it restating the body |
| Claude supervisor launch contract | tells the model to invoke `cas-supervisor` (body already injected), `cas-supervisor-checklist` (6,304 B body) and `cas-codebase-design` (6,627 B) → up to ≈4.9 k tok at launch, ≈1.6 k of it a duplicate |
| task-verifier agent body | 27,671 B ≈ 6.9 k tok **per verification spawn** |
| In-scope bytes (canonical flavour) | ≈97 KB skills/references + 38.6 KB agents; ×3 flavours on disk |
| Findings | **P0 25 · P1 34 · P2 27 · P3 ≈25** |
| Achievable always-loaded saving | ≈ −555 tok per Claude worker session, ≈ −300 tok per supervisor session |
| Achievable on-demand / per-spawn saving | ≈ −3.2 k tok per task-verifier spawn; ≈ −3.5 k tok across supervisor references; ≈ −2 k across worker references |

## Scope and method

| Item | Value |
|---|---|
| Skills | `cas-supervisor.md` + 13 references, `cas-supervisor-checklist.md`, `codex/skills/cas-codex-supervisor-checklist.md`, `cas-worker.md` + 4 references, `cas-task-tracking.md`, `cas-search.md`, `cas-memory-management/` (SKILL + 5 refs), `verify-before-claim/SKILL.md` |
| Agents | `agents/{task-verifier,session-summarizer,duplicate-detector,learning-reviewer,rule-reviewer}.md`, `codex/agents/factory-supervisor.md` |
| Parity | every file diffed against its `codex/` and `grok/` twin after prefix normalisation (`mcp__cas__` → `mcp__cs__` / `cas__`) |
| Ground truth | dispatch arms `cas-cli/src/mcp/tools/service/mod.rs`; schemas `cas-cli/src/mcp/tools/types/*.rs`, `crates/cas-mcp/src/types{,/ops_secondary}.rs`; close gates `mcp/tools/core/task/lifecycle/close_ops.rs`; factory ops `service/factory_ops.rs`; PreToolUse `hooks/handlers/handlers_events/pre_tool.rs`; launch contracts `crates/cas-pty/src/pty.rs`, `ui/factory/app/mod.rs`; lane registry `crates/cas-factory/policy/lane-registry.toml`; `cas <cmd> --help` on 3.31.0 |
| Prior review | `docs/analysis/2026-09-02-builtin-skills-review.md` — status table at the end; fixed items are not re-reported |
| Best practice | Exa research, 15 cited principles (§ Best-practice yardstick) |
| Method | five read-only sub-audits (supervisor ×2, worker, agents, task/search/memory); every P0 below was re-checked by the lane owner against the cited line before inclusion |

Cross-lane P0s referenced, not re-reported: **bundled non-SKILL.md files never refresh after
first install** (`sync_builtin_detailed`) — evidence from this lane: the installed
`~/.claude-daniel@petrastella.io/skills/cas-supervisor/references/code-review-queue.md` still
exists although the source file was removed; **Grok in cas-src resolves skills from
`.claude/skills`**, so every Grok-flavour finding below is moot in cas-src until that lands.

## SessionStart measurement and pinning contract

Injection path: `cas-core hooks/context/coordination.rs:227-261 inject_role_guidance` →
`HooksConfig::{supervisor,worker}_guidance()` (`config/access/hooks_traits.rs:52-57,101-106`) →
`builtins.rs:3532-3542` → `extract_body(SUPERVISOR_GUIDE|WORKER_GUIDE)` (always the Claude file;
prefix remapped once at assembly). Budget: `SESSION_START_BUDGET_BYTES = 9*1024`
(`hooks/handlers/session_budget.rs:57`); protected segments (role guidance) are never degraded,
so every byte of body growth is paid by degradable evidence sections (issue triage, ready tasks,
memories).

| Body | File bytes | Injected body | ≈ tok | Cap / floor | Headroom |
|---|---|---|---|---|---|
| Claude `cas-supervisor.md` | 6,766 | 6,567 | 1,641 | soft 8,000 / hard 8,192; remainder ≥ 2,400 | **249 B** (remainder floor) |
| Codex `cas-supervisor.md` | 6,665 | 6,466 | 1,616 | (not injected; mirror) | — |
| Grok `cas-supervisor.md` | 6,756 | 6,557 | 1,639 | (Grok ignores SessionStart stdout) | — |
| Claude `cas-worker.md` | 8,239 | 7,989 | 1,997 | ≤ 8,000; ≥ 1,024 below 9,216 | **11 B** (component cap) |
| Codex / Grok `cas-worker.md` | 8,229 / 8,189 | 7,979 / 7,939 | ≈1,990 | mirror | — |
| `cas-supervisor-checklist.md` | 6,487 | 6,304 | 1,576 | on demand (named in launch contract) | — |

Operator guidance on record: keep the supervisor body ≤ ~6,450 B (it is **117 B over** that
target today) and worker ≤ 8,000 B.

**Tests that pin `cas-supervisor.md` / `cas-worker.md` text or size** — any fix plan must run all
of these in one pass (no-fail-fast), not just Scoped Validation:

| Test | Location | Pins |
|---|---|---|
| `test_supervisor_guidance_loads`, `_hard_rules`, `_no_checklist` | `builtins.rs:3814,3839,3993` | keywords AskUserQuestion, SendMessage, "Never close", "Never implement", "Drive to the exit", exit-ladder rungs |
| `test_supervisor_guidance_under_8kb` | `builtins.rs:4018` | < 8,192, ≤ 8,000, ≥ 512 headroom |
| `supervisor_guidance_leaves_room_for_the_rest_of_the_session_start_payload` | `builtins.rs:4054` | remainder ≥ 2,400 B |
| `the_codex_worker_matrix_still_fits_the_session_start_budget` | `builtins.rs:4081` | guidance + Codex note leaves ≥ 1,400 B |
| `test_codex_supervisor_guidance_mirrors_tiering_rule` | `builtins.rs:3914` | "Tier every spawn", "never fleet-default", "Registry lanes", lane triples, "standing suspension" |
| `test_supervisor_bodies_normalized_consistent_across_harnesses` | `builtins.rs:3945` | byte-equality after prefix + checklist-line substitution; hetero heading |
| `role_entrypoints_share_authoritative_acceptance_and_wake_policy` | `builtins.rs:3587` | wake-policy substrings; banned stale ACK phrases |
| `test_worker_guidance_loads`, `test_worker_guidance_under_session_start_budget` | `builtins.rs:4113,4156` | ≤ 8,000 B, ≥ 1,024 below budget; structured-state/context-budgeting stay in details.md |
| `test_skills_document_context_budgeting_cas_5787` | `builtins.rs:4182` | "## Context budgeting", "Immutable Core", `project_session_start_truncation.md` |
| `test_worker_skills_carry_backgrounding_mandate_cas_b4921`, `_require_cas_src_surface_checklist`, `_pin_return_contract_and_silent_execution_cas_0de3`, `_teach_no_rust_build_rule_cas_4cbb`, `test_cas_worker_skill_documents_close_gate` | `builtins.rs:4467,4563,4597,4651,4701` | worker body phrases incl. ticket label `cas-2327` |
| `test_taste_routes_match_registry_in_all_supervisor_readers`, `test_supervisor_model_selection_reference_registered_and_mirrored`, `test_supervisor_rubric_recipes_and_reference_twins_stay_normalized` | `builtins.rs:6869,6921,7134` | lane triples in body/refs match registry |
| `test_worker_merge_state_guidance_…`, `_toolsearch_two_step_…`, `_never_self_dispatch_…` | `builtins.rs:7327,7409,7482` | worker body phrases (self-dispatch ×5) |
| task-verifier marker test | `builtins.rs:6270-6278` | **pins the wrong `files_reviewed=` marker** (L2-P0-01) |
| `factory_codex_skill_guardrails` (9 tests) | `cas-cli/tests/factory_codex_skill_guardrails.rs:99-600` | core workflow, pane budget, max-effort, operator authority, lifecycle contract, epic-driving compactness |
| `builtin_doc_hygiene_test::supervisor_guidance_drives_each_turn_to_a_named_exit_rung` | `tests/builtin_doc_hygiene_test.rs:53` | exit ladder |
| `builtin_flavor_drift_test` (incl. liveness contract `:1563`, decision table `:1124-1150`) | `tests/builtin_flavor_drift_test.rs` | three-way parity |
| `agent_definition_contract_test::epic_walk_is_one_concurrent_pass_in_every_harness` | `tests/agent_definition_contract_test.rs:176` | epic walk wording |
| `issue_intake_directive_test` | `tests/issue_intake_directive_test.rs:78-100` | registry keys incl. deprecated `issues.components.mecha_cassy` (L2-P0-18) |
| `session_start_issue_triage_test` | `tests/session_start_issue_triage_test.rs` | assembled payload keeps issue titles (fails on body growth) |
| `verify_before_claim_skill_test` | `tests/verify_before_claim_skill_test.rs:79,113,129` | v1 narration, worker pre-close sentence, close-gate link |
| `mcp_action_surface_test` | `tests/mcp_action_surface_test.rs:268-341` | "## Valid Actions" shape, memory Request Fields order |
| `builtin_skill_description_test` | `tests/builtin_skill_description_test.rs` | descriptions, `/plan` ban |
| `--lib cli::factory::parity`, `session_budget` unit tests | `cli/factory/parity.rs:630-670`, `session_budget.rs:532,659` | assembled payload ≤ 9,216 B |

## Best-practice yardstick (Exa, 2025-2026 sources)

1. Smallest set of high-signal tokens; always-loaded text is a budget
   ([Anthropic, context engineering, 2025-09-29](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)).
2. Heuristics at the right altitude, not laundry lists of edge cases (same source).
3. A prompt rule is a request, a hook is enforcement — "If a rule must hold every time, make it
   a hook rather than a prompt instruction" ([Claude Code features overview](https://code.claude.com/docs/en/features-overview)).
4. Dial back aggressive language; give the reason instead
   ([Claude prompting best practices](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices)).
5. Opus 5: remove verification instructions carried over from earlier models; they cause
   over-verification ([Prompting Claude Opus 5](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-5)).
6. Each delegated task needs objective, output format, tool guidance, boundaries
   ([Anthropic multi-agent research system, 2025-06-13](https://www.anthropic.com/engineering/multi-agent-research-system)).
7. Workers store output externally and pass lightweight references back (same source).
8. Skills: SKILL.md < 500 lines / < 5 k tok, references one level deep, `name` = directory
   ([agentskills.io spec](https://agentskills.io/specification); [Anthropic skill best practices](https://docs.anthropic.com/en/docs/agents-and-tools/agent-skills/best-practices)).
9. Subagent frontmatter: only `name`, `description` required; `tools`, `disallowedTools`,
   `model` (incl. `inherit`), `effort`, `maxTurns`, `skills` exist today
   ([Claude Code sub-agents](https://code.claude.com/docs/en/sub-agents)).
10. The verifier is the real spec: deterministic checks first, then one rubric judge
    ([Anthropic C compiler, 2026-02-05](https://www.anthropic.com/engineering/building-c-compiler)).

Implication applied below: rules the harness already enforces (`pre_tool.rs` denials, close
gates) should be one pointer line, not prose in always-loaded bodies; verifier prose that
duplicates Rust close gates is cost without enforcement.

## Findings

Columns follow the rubric. **Surface:** `always` (SessionStart body / launch contract /
agent-listing description), `per-invoke` (skill body or agent body per spawn), `on-demand`
(references). **Ax** = rubric axis (1 frontmatter · 2 description · 3 disclosure/size ·
4 wording · 5 procedure · 6 accuracy · 7 parity · 8 tokens). Paths are relative to
`cas-cli/src/builtins/skills/` unless they start with `agents/`, `codex/`, `crates/` or
`cas-cli/`. Every markdown fix lands in all three flavours.

### P0 — misleads an agent today

| # | Sev | Surface | file:line | Ax | Defect | Evidence | Fix | Δ tok |
|---|---|---|---|---|---|---|---|---|
| 01 | P0 | per-invoke (every verification) | `agents/task-verifier.md:378,385,392,399` | 6 | **Regression of 09-02 P0 #1.** All four verdict templates pass `files_reviewed=`; the tool's request struct field is `files`, no `deny_unknown_fields`, so the reviewed-files list is silently dropped on every verdict. The 09-02 review read the inner `VerificationAddRequest`, not the tool struct. | `crates/cas-mcp/src/types/ops_secondary.rs:353-396` `pub files: Option<String>`; mapping `service/worktree_verification_team_ops.rs:98` `files_reviewed: req.files`; wrong marker pinned at `builtins.rs:6274` | `files="…"` in the four templates ×3 flavours; change the pin to `files=`; optionally `#[serde(alias = "files_reviewed")]` on `VerificationRequest.files` | −10 |
| 02 | P0 | per-invoke (worker deep close) | `verify-before-claim/SKILL.md:27-28,42,72-73` | 6,4 | Proof examples are `cargo test`, `cargo build`, `./target/release/cas`; workers are denied every one of them, and `cas-worker.md:31-34` routes every deep close through this skill ("If you cannot do all four, you are not done", `:103`). A Rust-task worker either hits the denial or concludes it cannot close. | `hooks/handlers/handlers_events/pre_tool.rs:30-69` NO WORKER RUST BUILDS; close appends the deferred build-proof note itself (`close_ops.rs:7316-7327`) | Add a factory-worker row: Rust proof = `git diff --stat` + `rg` wiring evidence + non-Rust suites; Rust tests defer to the epic `ASSEMBLY_PROOF`. Label the cargo rows "supervisor / non-factory". | +60 |
| 03 | P0 | on-demand | `cas-supervisor/references/worker-recovery.md:82` | 6 | Dead-worker step "`shutdown_workers count=0`" shuts down **every** worker, live ones included. | `service/factory_ops.rs:2778-2785` (`limit == 0` → all known workers) | `mcp__cas__coordination action=shutdown_workers worker_names=<worker>` | +10 |
| 04 | P0 | on-demand | `worker-recovery.md:81` | 6 | "Release the dead worker's lease: `task action=release`" — release only succeeds for the lease owner; the supervisor gets "Failed to release task". | `crates/cas-store/src/agent_store/ops_task_leases.rs:164-174` (`owner_id == agent_id`); `reset` documented as the non-owner verb in the tool description | `mcp__cas__task action=reset id=<task-id>` | 0 |
| 05 | P0 | on-demand | `worker-recovery.md:137` | 6 | Near-limit example `coordination action=message` has no `summary` → rejected. | `agent_search_system/message.rs:644` "summary required"; `reference.md:156-164` already says so | add `summary="Context near limit — commit now"` | +8 |
| 06 | P0 | per-invoke | `cas-task-tracking.md:13` | 6 | The canonical create example omits `risk`; default `task_type=task` → "TASK CREATE REJECTED: risk is required". | `mcp/tools/types/task.rs:128-140`; called unconditionally `service/core.rs:249-257` | `action=create title=… description=… priority=2 risk=none` + one clause: risk is required for task/bug/feature; `blast-radius` needs `proof_targets` | +25 |
| 07 | P0 | per-invoke (Stop-hook job) | `agents/learning-reviewer.md:36` | 6 | Skill-create template omits `invocation` → always "invocation required"; if fixed as-is, defaults are `scope=global`, `draft=false`, i.e. an unreviewed skill goes live everywhere. | `service/core.rs:948-968` | `mcp__cas__skill action=create name=… description=… invocation="…" scope=project draft=true tags=from_learning source_ids=…` | +15 |
| 08 | P0 | per-invoke (Stop-hook job) | `agents/rule-reviewer.md:24,42` | 6 | "Promote: `rule action=helpful`" — `helpful` adds one vote; promotion only at `sync.promotion_threshold` (default 2) with zero harmful, so the "caused a real rejection" promote path never promotes, and the reviewer's own vote inflates the metric it judges by (`:51-52`). | `cas-cli/src/mcp/tools/core/rules.rs:37-41,235-256`; `settings.rs:1443` | State the vote semantics; call `helpful` at most once per rule; report whether the response says promoted — or add a Rust `promote` action | +40 |
| 09 | P0 | on-demand | `cas-worker/references/close-gate.md:61`, `recovery.md:19` | 6 | `completion_receipt` is documented as moving the task to `awaiting_merge`; a new receipt starts `AwaitingVerification` (projects to in-progress + pending verification) and is **rejected unless the source tip is already merged**, which the doc never says. `artifact_path` omitted. The 09-02 "After" wording would have taught the same wrong state. | `close_ops.rs:4166-4183` (`DELIVERY RECEIPT REJECTED … until the current source tip is merged`; `initial_state = AwaitingVerification`); `crates/cas-types/src/delivery.rs:27` | "Only after your tip is merged: … persists an immutable delivery, releases your lease, and leaves the task in progress with verification pending." Add `artifact_path?`. | 0 |
| 10 | P0 | always (worker) | `cas-worker.md:37-39` | 6 | MERGE REQUIRED step says "push the branch" unconditionally; contradicts `:28-29` and `close-gate.md:42`; the push is denied for `local_merge`. Same unconditional push in all worker launch contracts (`pty.rs:20,41,313+`, L3 lane). | `pre_tool.rs:210-226` LOCAL-MERGE DELIVERY denial | "push the branch (`push_branch` only; for `local_merge` send the SHA without pushing)" | +12 |
| 11 | P0 | on-demand | `cas-supervisor/references/workflow.md:282-283,278` | 4,6 | "Close approved tasks in a second parallel pass" directs the supervisor to break the always-loaded hard rule "Never close tasks for workers" (`cas-supervisor.md:17`); close does not block it. | `close_ops.rs:6014,6222` supervisor-closes-worker path | "Merge each parked lane (`worktree_merge`), message each worker to re-close, then reassign"; `:278` → "after a worker's close" | −20 |
| 12 | P0 | on-demand | `workflow.md:288` | 6 | Phase 4 "Verify all tasks closed: `task action=list status=open epic=<id>`" misses `in_progress`, `blocked`, `awaiting_merge` → false "all closed". | statuses `crates/cas-types/src/task.rs:19-38` | `mcp__cas__coordination action=epic_status id=<epic-id>` (same source as the close gate) | 0 |
| 13 | P0 | on-demand | `workflow.md:36` | 6 | `search action=search … scope=code` — `scope` accepts only global/project; `code` is a `doc_type` alias, so this searches everything. | `mcp/tools/core/search.rs:148,155-159` | `doc_type=code` (symbols) or `doc_type=code_file` | 0 |
| 14 | P0 | on-demand | `workflow.md:249-258` | 6 | Says `sync_all_workers` replaces messaging each worker and `force=true` covers a worktree "whose assignee is mid-task"; supervision-live worker worktrees are **always** skipped, force covers only dirty trees and stale records. | `factory_ops.rs:7627-7644`; tool description `service/mod.rs:541` | "Rebases idle or stale worktrees only; live workers are always skipped — tell each to rebase at next task start. `force=true` covers a dirty tree or stale record." | +15 |
| 15 | P0 | on-demand | `workflow.md:133`, `worker-recovery.md:169` vs `cas-supervisor-checklist.md:21-23` | 4,6 | References tell the supervisor to `~/.cargo/bin/cargo build --release` and restart `cas serve`; the checklist says never kill/restart `cas serve` from the active MCP session — ask the operator. The build line is also cas-src-only in a universal builtin. | quoted lines; installed universally `builtins.rs:222` | "Binary stale? Stop and ask the operator to rebuild and reconnect MCP (preflight.md)." | −30 |
| 16 | P0 | on-demand | `cas-supervisor/references/reference.md:87` | 6 | `config_dir` documented as "Claude-only, Codex/Grok ignore it"; Codex is supported via `CODEX_HOME`, only Grok warns. The Rust schema string (`ops_secondary.rs:1137`) is equally stale (L3). | `factory_ops.rs:50-65,121-170,2416-2423` | "`CLAUDE_CONFIG_DIR` for Claude, `CODEX_HOME` for Codex; explicit wins, else captured from the requester; Grok has none and warns." | −20 |
| 17 | P0 | on-demand | `reference.md:47-48` | 6,4 | "Valid coordination actions (do not invent others)" omits `recycle_worker` and `restart_spawn_queue`, while `worker-recovery.md:111` tells the supervisor to run `restart_spawn_queue` — the prohibition forbids a real, recommended action. | `service/mod.rs:692,717` | add both; drop "do not invent others" (the server already returns the valid list on an unknown action) | +8 |
| 18 | P0 | always (both bodies) + on-demand | `cas-supervisor.md:67`, `cas-worker.md:72`, `cas-supervisor/references/filing-cas-bugs.md:19,33`, `reporting-and-routing.md:13` | 6 | Bug-routing key `issues.components.mecha_cassy` is deprecated ("use issues.components.violet … accepted for one release"). Pinned by `issue_intake_directive_test.rs:93`; the `cas init` CLAUDE.md block carries it too (cross-lane). | `cas-cli/src/config/access/mod.rs:9-13`; `config/meta/seed/issues.rs:38-61`; `cas config get` prints the warning | switch to `issues.components.violet` in all flavours + the test in the same change; rename prose "MechaCassy" per the seed label | 0 |
| 19 | P0 | per-invoke | `cas-supervisor-checklist.md:103,108` (codex `:107,112`) | 6 | Says the stranded-branch epic-close gate "cannot be waived … regardless of supervisor overrides"; `stranded_branch_override` exists and is honoured for a live supervisor. `:108` repeats `:103`. | `mcp/tools/types/task.rs:313-321`; `close_ops.rs:5529-5560` | "…refused unless a live registered supervisor passes `stranded_branch_override="<inspection narrative>"`"; delete `:108` | −75 |
| 20 | P0 | always (supervisor) + on-demand | `cas-supervisor.md:23`, `references/model-selection.md:192,199`, `workflow.md:75-79`, `codex/agents/factory-supervisor.md:18` | 6,4 | "Pass explicit `cli=`/`model=`/`effort=`" on every spawn contradicts the generated `lane=<lane>` mode; the fallbacks promised at `model-selection.md:13-17,51` fire **only** in lane mode, so a supervisor following the body never gets them and the "(fallback: …)" labels on explicit recipes are false. | `ops_secondary.rs:746-751,1102-1106` (lane excludes cli/model/effort); `factory_ops.rs:958,2323-2345` (lane resolution + loud fallback), `:2395-2407` (explicit path, no fallback), `:1278-1279` (no untiered warning with lane) | Body: "Pass `lane=<light\|standard\|taste\|heavy>` (preferred) or a full explicit recipe to force one model — never both." Update pins `builtins.rs:3914-3935,6869,7134`. | −40 |
| 21 | P0 | on-demand | `model-selection.md:186` | 6 | "docs-only → light" contradicts `:13,16,180` (public docs/skills/prompts → taste) and conflates task `depth=light` with the light lane. | `mcp/tools/types/task.rs:259-264` (`depth` is execution depth) | "mechanical, non-public docs → light; public docs, skills, prompts → taste"; drop `depth=light` | 0 |
| 22 | P0 | on-demand | `cas-supervisor/references/planning.md:118,120` | 6,4 | Per-merge "re-run touched modules on the merged tree" contradicts the build-once rule (`cas-supervisor.md:29`, `workflow.md:184-186`). | quoted lines | "At merge: read the diff, check contracts and lane CI. The single build + test runs at Phase 4." | −40 |
| 23 | P0 | on-demand | `cas-memory-management/references/schema.yaml:14,54`, `body-templates.md:84-85` | 6 | **Partial regression of 09-02 P0 #13.** "The single entry-type enum" lists four values; SKILL.md lists five; `handoff` is live (special-cased, skips overlap). Unknown values silently become `learning`, so "must be one of" is also false. | `mcp/tools/core/memory.rs:453-458,513`; `crates/cas-mcp/src/types.rs:35-38`; `cas-types/src/entry.rs:150-155` | add `handoff`; ":54 → unknown values are stored as learning; handoff supersedes the role's previous handoff and skips overlap" | +15 |
| 24 | P0 | always (worker) + on-demand | `cas-worker.md:70-74` vs `:102-104`; `filing-cas-bugs.md` (silent) | 6,4 | Body routes Cassy bugs to `issues.components.cassy`, then recommends `system action=report_cas_bug`, which files to `issues.repo` whenever set — in a downstream project a Cassy bug lands in that project's tracker. `:102-103` ("stay in this repository … fix them here") is cas-src-only text injected into every project. | `agent_search_system/system.rs:263-277`; test `configured_issue_repo_is_the_only_filing_target` `:968-983` | Replace `:102-106`: "In cas-src fix Cassy bugs via an assigned task; elsewhere file in `issues.components.cassy` (`report_cas_bug` files to `issues.repo`)." Better: Rust task to route `report_cas_bug` via the cassy component. | −40 |
| 25 | P0 | on-demand | `cas-worker/references/recovery.md:44` | 4 | "Prevention lives in discipline.md: report headroom in every milestone note" contradicts `discipline.md:3-6`, the body `:66`, and every spawn contract (report only below 20 %). | quoted lines | "Prevention: below 20 % headroom, checkpoint (commit, push/park, handoff note, respawn request)." | −10 |

### P1 — routing, format, budget, architecture

| # | Sev | Surface | file:line | Ax | Defect | Evidence | Fix | Δ tok |
|---|---|---|---|---|---|---|---|---|
| 26 | P1 | always (worker) | `cas-worker.md` (whole body) | 3,8 | Injected body is 7,989 B vs the 8,000 B test cap — **11 B**. Every P0 fix above that touches the body fails `test_worker_guidance_under_session_start_budget` unless something is cut first. | `builtins.rs:4156-4172`; measured with an `extract_body` replica | Land #27 (and #33) first, in the same change as any body fix | — |
| 27 | P1 | always (worker) | `cas-worker.md:115-133` | 6,8 | cas-src surface checklist (1,149 B, 14 % of the body) injected into every project's workers; pinned including the ticket label `cas-2327`. | `test_worker_skills_require_cas_src_surface_checklist` `builtins.rs:4563-4588` | Move to a "cas-src extras" section of `references/close-gate.md`; one pointer line; retarget the pin | −260 |
| 28 | P1 | always (supervisor) | `cas-supervisor.md` (whole body) | 3,8 | 6,567 B is 117 B over the operator's ≤ 6,450 B target and 249 B above the 2,400 B remainder floor; the 3.17.3 release failed twice on this margin. | `builtins.rs:4054`; memory "skill-text-pins" | Cut #33-#36 items (≈ −1.2 KB) before any addition | — |
| 29 | P1 | always (both) | `cas-supervisor.md:17,23,25,28,34,53,65-68`; `cas-worker.md:31,149,151,153,155` | 3,7 | Links are written `cas-supervisor/references/…` / `cas-worker/references/…` — correct in the source tree (`skills/cas-supervisor.md`), **broken in the installed layout** (`skills/cas-supervisor/SKILL.md` + sibling `references/`). `cas-worker.md:33,146` use installed-relative `../…`, so one file mixes two frames. | `builtins.rs:142,238-255` install paths; `ls ~/.claude-daniel@petrastella.io/skills/cas-supervisor/` → `SKILL.md references` | `references/<file>.md` in all three flavours; add a link-resolution test over the installed catalog | −30 |
| 30 | P1 | always (supervisor) | `cas-supervisor.md:13-15` | 4 | Three hard-rule bullets for calls the harness already denies (SendMessage, AskUserQuestion, `Agent(isolation: "worktree")`). Rubric: harness-enforced rule restated in always-loaded text. | `pre_tool.rs:111-129,146-160,173-177` | One line: "Harness-denied: SendMessage, AskUserQuestion, Agent(isolation=worktree) — use `coordination action=message … summary=…`, ask in your reply, `spawn_workers`." Keep pinned keywords. | −40 |
| 31 | P1 | always (worker) | `cas-worker.md:26-27,137`; `close-gate.md:42,128`; `discipline.md:8-28` | 4 | No-Rust-build, workspace-contract and local-merge-push denials restated as prose (partly pinned). | `pre_tool.rs:30-69,210-226,386-395` | One pointer line in the body; keep pinned phrases; drop the rest | −75 |
| 32 | P1 | always (Claude supervisor launch) | `crates/cas-pty/src/pty.rs:284-300` (`claude_supervisor_contract`) — L3 surface, recorded here for its cost to this lane's files | 3,8 | Launch contract says "Use skills cas-supervisor, cas-supervisor-checklist, and cas-codebase-design" to a Claude supervisor whose SessionStart already injected the `cas-supervisor` body — invoking it re-loads ≈1.6 k tok; the three together ≈4.9 k tok at launch. | quoted string; `ui/factory/app/mod.rs:2533-2560` | Claude contract: "Your guidance is already loaded; run `cas-supervisor-checklist` once at session start." | −1,600 (per supervisor launch) |
| 33 | P1 | always (Claude worker) | `cas-worker.md:12-13,17-21,26-27,37-43,64-66,93,100,112-113,139-141` vs `claude_worker_contract` (`pty.rs:313-352`, 3,976 B) | 3,8 | ≈1.8 KB of the same rules delivered twice per Claude worker (body + launch contract). Constraint: the body survives `/clear`; the contract is parity-pinned (cas-0263). | span table in the worker sub-audit; `missing_contract_elements` | Cut from the body only harness-enforced items (#31) and merge `:64` with `:139-141`; move the rest of the dedupe to the L3 contract | −100 |
| 34 | P1 | on-demand | `worker-recovery.md:3-20,22-55,69-84,153-154` | 5,6 | Three dead-worker procedures; the third (`:69-84`) combines #03, #04, an untiered spawn, and "cherry-pick salvageable work to the base branch" (contradicts `epic-driving.md:5`). | quoted lines | Delete `:69-84`; "Silent worker: `worker_status`, then the is-wedged triad; salvage with `worktree_merge id=<worker> task_id=<task>`." | −300 |
| 35 | P1 | on-demand | `worker-recovery.md:26-34`, `:120-134`, `:136-140,152` | 6 | `is-wedged` table lists five states, omits `approval-hang` (exit 5 → `cas factory approve/deny`); context bands in absolute tokens while code classifies percent of window (0-49/50-79/≥80) with a different output line; near-limit remedy ignores `recycle_worker` (which `worker_status` itself recommends). | `cli/factory/wedged.rs:367-386`; `factory_ops.rs:9932-9955,9985,2982-3076` | add the row; percentage bands + real line; step 4 → `coordination action=recycle_worker target=<worker>` | +15 |
| 36 | P1 | on-demand | `worker-recovery.md:178-208`; `cas-worker/references/recovery.md:75,151-267`; `details.md:36-38` | 6 | ≈7.6 KB of stuck-cargo triage built on workers running cargo (now denied; `recovery.md:165` "Re-run it"); sccache cited as "Phase 2"; `workflow.md:114` inverts `worker_build_jobs`/`cargo_build_jobs` canonical/alias; bare `git stash`/`pop` in worker recovery despite a stash stack shared by every worktree. | `pre_tool.rs:30-69`; `config/settings.rs:453` | Move build triage to the supervisor tree (assembly owner) as ≤ 10 lines; WIP-commit instead of stash | −1,850 |
| 37 | P1 | on-demand | `workflow.md:118,135,157,262-263,312,316,322`; `worker-recovery.md:173` | 5,4 | "Verify workers appear in TUI" (agent cannot see the TUI); "ask the supervisor" in the supervisor's own guide; Phase 4 lane-landing order circular with the gate; `:157` "diff review after merge" vs `:182-186` "before landing"; duplicate `shutdown_workers count=0` (`:316` vs `:324`). | quoted lines | `worker_status summary_mode=true`; "ask the operator"; reorder; delete `:157`,`:316` | −45 |
| 38 | P1 | always + on-demand | `cas-supervisor.md:53,60`; `workflow.md:5-8,60-62,225`; `epic-driving.md:7` | 5,6 | Spawn examples untiered (`:53` vs rule `:23`) and without `isolate=true` (default false → NON-ISOLATED warning on every receipt); `workflow.md:5-8` calls shared mode a neutral "simpler setup"; epic-driving says one spawn per task with `task_id`, workflow says batch then `update assignee=`. | `factory_ops.rs:2086,2117,2481-2485` | Every example: `lane=<lane> isolate=true task_id=<id>`; state the single dispatch pattern once in the body | +10 |
| 39 | P1 | always + on-demand | `cas-supervisor.md:23` | 6 | "`max` only … where the recipe lists it (Fable, Opus, Astra, Sol)" reads as allowing Opus 5.5 — the model of three lanes — which rejects `max`; omits GPT-6 Luna, which allows it. | `lane-registry.toml:42` (`claude_opus_5_5 allowed_efforts=["low","high"]`), `:33` | "(Fable 5.1, Opus 5, Astra, Sol, GPT-6 Luna — not Opus 5.5)" or pointer only | 0 |
| 40 | P1 | on-demand | `model-selection.md:106,194` | 6 | Lists `gpt-5.6-terra` as "Accepted" (spawns reject suspended recipes); calls Luna/xhigh the "standard route" (it is light primary / standard fallback). | `lane-registry.toml:125-151`; test `factory_ops.rs:12163-12175` | remove terra; "Luna/xhigh (light route)" | −85 |
| 41 | P1 | on-demand | `cas-supervisor/references/reminders.md:49-51` | 6 | `remind_event=task_completed` example has no filter → fires on **any** task completion. | `queue_and_events.rs:8105-8107` "No filter = match any event" | add `remind_filter='{"task_id":"<task-id>"}'` | +10 |
| 42 | P1 | on-demand | `planning.md:63,79,86-97` | 6 | Template-to-field map omits `risk`/`proof_targets` (required on create); execution_note "One of …" omits `no-code`. | `mcp/tools/types/task.rs:176-189,246-250` | add the row and the value | +40 |
| 43 | P1 | per-invoke | `cas-supervisor-checklist.md:11-23` (codex twin same) | 5,6 | Step 0 is a cas-src-only manual SHA comparison (`cargo build`, `cas-cli/src` paths) in a builtin every project installs; `preflight.md:57-60` says downstream HEAD is never compared; `cas factory preflight` already performs the check. | `factory_preflight.rs:22-25,1161-1167` | "Run `cas factory preflight`. Nonzero → fix the named finding. Stale binary → ask the operator to rebuild and reconnect MCP." | −300 |
| 44 | P1 | per-invoke | `cas-supervisor-checklist.md:92` | 6 | Per-task review gate "Tests exist and pass" contradicts build-once/`ASSEMBLY_PROOF` (body `:29`, `workflow.md:184-186`). | quoted lines | "Tests added/updated for the change; run at assembly." | 0 |
| 45 | P1 | on-demand | `intake.md`, `planning.md` | 3 | Orphaned references: nothing links either file (only their catalog registrations). | `rg -n "intake.md\|planning.md" cas-cli/src/builtins` → registrations only | Add to the body's reference line: "intake gate: intake.md · spec template: planning.md · preflight: preflight.md" | +30 |
| 46 | P1 | on-demand | `model-selection.md:1-5`, `planning.md:1-5`, `cas-worker/references/close-gate.md:1-5` | 1 | References carry frontmatter (`name`/`description` on the first two) — Grok/Codex walk recursively and may register them as skills (rubric Axis 1 P1). | quoted frontmatter; no test requires it (`builtins.rs:6329-6360` is hash-based) | delete the blocks | −95 |
| 47 | P1 | on-demand | `reporting-and-routing.md:9` | 6 | Describes a `cas-cut-release` fallback that posts through the direct MechaCassy MCP; no such stage exists. | `cas-cut-release/SKILL.md:47-56`; only a failure-log narrative mentions it | delete the sentence | −45 |
| 48 | P1 | always (Codex supervisor) | `codex/agents/factory-supervisor.md:3,12,42-44` | 1,2 | Description still a role summary (09-02 P1 #7 unfixed); no `model:`; Cassy never references the file (the Codex intro names skills only); forbids `/cas-start` etc. that exist in no catalog; `:42-44` repeats `:13`. | `ui/factory/app/mod.rs:2547-2553`; UNVERIFIED whether Codex CLI auto-loads `.codex/agents/*.md` | Wire it into the Codex intro or retire it; if kept: "Use when running as the Codex factory supervisor: …", `model: inherit`, drop dead lines | −85 |
| 49 | P1 | per-invoke (verification) | `agents/task-verifier.md:12-24` | 5,6 | "Close-Path Error Detection" instructs the closer, not the verifier (which never calls close); the rejections the verifier can hit (`Verifier handoff rejected`, `Verifier capability rejected`, `Verification authority rejected`) are absent, and the section contradicts fail-closed `:468-471`. Pinned marker "Close-Path Error Detection" (`builtins.rs:6276`). | `verification_tools.rs:165,267-283,628-680`; close strings `close_ops.rs:5173,6249,5466,6862` | Replace with: "If `verification action=add` returns any of those three messages, stop, do not retry, quote it verbatim." Update the pin. | −220 |
| 50 | P1 | per-invoke (verification) | `task-verifier.md:144,154,279`; `:109,123` vs `:475,478`; `:85-87` | 5,6 | `HEAD~10` diff base (still present) pulls unrelated or misses commits; reject policy contradicts itself (Step 0B: don't reject on keywords vs "any placeholder language = reject", "if in doubt, reject"); NOT-EXERCISED rows' effect on approval unstated. | `cas-types/src/task.rs:413,428,431` (`work_target`, `files_changed`, `commit_hash`) | Base = `git merge-base HEAD <work_target>`; prefer `deliverables.files_changed`; scope `:475` to changed code; replace `:478` with "reject only naming the unmet AC item"; decide NOT-EXERCISED policy (operator call) | 0 |
| 51 | P1 | per-invoke (Stop job) | `agents/session-summarizer.md:11-14`; `duplicate-detector.md:11,30` | 5,6 | Light-lane jobs run with identity env stripped (`light_lane.rs:36-91`): summarizer's `task action=mine` resolves the wrong caller and it never reads the transcript path the prompt passes; duplicate-detector ignores the 15 IDs the hook supplies, fetches `recent limit=50` (timeout risk in a 300 s job), and its "flag for review" step needs a task id it doesn't have. | `stop_flow.rs:620-680`; `session_stop/mod.rs:405-417`; `task_claiming.rs:1031-1047` | Summarizer: read the transcript path; `task action=list status=in_progress`. Detector: process exactly the job IDs; print `UNCERTAIN <keep> <dup>: reason` lines | 0 |
| 52 | P1 | per-invoke | `cas-task-tracking.md:22,27`; `cas-worker/references/details.md:61` | 6 | "Blocked" → `list status=blocked` returns only explicit-status rows; dependency-blocked work is `action=blocked`. Note types omit `platform_proof`, which the close gate requires for `risk=platform`. | `service/mod.rs:374` → `core/task/query.rs:494-519`; `notes.rs:72-80`; `close_ops.rs:1654,1768` | `task action=blocked`; add `platform_proof` | +25 |
| 53 | P1 | per-invoke | `cas-search.md:15-16` | 6 | 09-02 P1 #15 still present: `doc_type` omits `spec`, `artifact`; unknown values silently search all; `scope`/`tags` unmentioned. | `mcp/tools/core/search.rs:141-176` | one line listing all values + scope/tags | +35 |
| 54 | P1 | on-demand | `cas-memory-management/references/overlap-detection.md:41-43`, `response-shapes.md:25` | 6 | Describe cap handling the MCP caller doesn't do: a capped candidate gets no link on either entry and is absent from `related_memories`. | `mcp/tools/core/memory.rs:683` | reword both | −5 |
| 55 | P1 | on-demand | `cas-worker/references/close-gate.md:11-22` vs `cas-worker.md:31` | 5 | Clean-tree receipt applies to every close incl. light; the body routes only deep tasks to close-gate.md. | quoted lines | body step 7: "Every close: `git status --porcelain` empty and HEAD is the commit you claim." | +22 |
| 56 | P1 | on-demand | `cas-worker/references/details.md:45-47` | 6 | "`vercel env pull` … real prod credentials" contradicts `:69` and the launcher, which strips `VERCEL_TOKEN`/`NEON_API_KEY` from workers. | `pty.rs` `PROTECTED_OPERATOR_ENV` | delete | −83 |
| 57 | P1 | on-demand | `epic-driving.md:10-13`; `workflow.md:117,295,308`; `epic-flow-walk.md:49`; `filing-cas-bugs.md:24-29`; `planning.md:120` | 6 | cas-src-only content (release prebuild, `release/vX-prepare`, `scripts/refresh-worker-build-cache.sh`, `cargo nextest run -p cas`, Richards-LLC issue links) in builtins installed in every project. | `builtins.rs:222` universal install | "the project's assembly gate command"; move cas-src specifics to repo CLAUDE.md/docs | −220 |
| 58 | P1 | always (Claude agent listing) | `agents/{learning-reviewer,rule-reviewer,duplicate-detector,session-summarizer}.md` (Claude + Grok flavours) | 2,7 | The Stop hook runs the **Codex** bodies via the light lane (`stop_flow.rs:620-651`, `include_str!`); the Claude/Grok copies are never spawned but add four Agent-tool listing entries per Claude session with "Spawned by Stop hook" descriptions (duplicate-detector lacks "Do not invoke directly"). On a Claude fallback (`routing.rs:1378`) the Codex bodies' `mcp__cs__` names would not exist. | quoted spawn sites; this session's agent list shows all four | Stop syncing them as subagents; keep one job body per agent and remap the prefix at prompt build | −150 per Claude session |
| 59 | P1 | per-invoke | `task-verifier.md` frontmatter | 1 | No `tools:` list — the verifier inherits Edit/Write/Agent while the body only asks it not to edit (`:84`) or rerun QA (`:49-50`). | Claude Code sub-agents spec (tools / disallowedTools) | `tools: Read, Grep, Glob, Bash, mcp__cas__task, mcp__cas__verification, mcp__cas__rule, mcp__cas__search, mcp__cas__coordination` (prefix per flavour) | +30 |

### P2 — efficiency, duplication, structure (ranked by Δ × multiplier)

| # | Sev | Surface | file:line | Ax | Defect | Evidence | Fix | Δ tok |
|---|---|---|---|---|---|---|---|---|
| 60 | P2 | per-invoke (every verification) | `agents/task-verifier.md` | 3,8 | 27,671 B ≈ 6.9 k tok per spawn. Demo/epic evidence `:26-93` (4.5 KB) applies only with a `demo_statement`; Phase 2 rubric `:298-370` (3.3 KB) is generic; four near-duplicate verdict templates `:374-400` (2.65 KB); posture SKIP bullets and confidence table restate Rust-enforced or display-only behaviour (`close_ops.rs:7032-7050`, `verification_tools.rs:528,876`). | section byte counts | Demo section → on-demand reference read only when `demo_statement` non-empty; one template; 6-line rubric | −3,200 per spawn |
| 61 | P2 | always (supervisor) + on-demand | `cas-supervisor.md:23,55-63`; `workflow.md:75-80,104-105`; `reference.md:84,89`; `model-selection.md:13-17,51,106-107,136,194` | 3,8 | Lane matrix restated by hand ≈ 8 times (09-02 finding, grown); "Terra suspended" ×5. | byte spans in the supervisor sub-audits | Keep body `:23` (pinned) and the two generated blocks; delete the "Heterogeneous Teams" section (update `CANON_HETERO` pin) and every prose copy | −80 always, −1,050 on-demand |
| 62 | P2 | always (supervisor) | `cas-supervisor.md:67`, `:70-72` | 8 | Bug-registry sentence (455 B) duplicates the CLAUDE.md block `cas init` injects into every project (`cli/init/docs_and_skill.rs:22`); "Context budgeting" (188 B) is maintainer text citing a memory file absent from the repo (`project_session_start_truncation.md`), and the budget is enforced by tests. Both pinned (`issue_intake_directive_test`, `test_skills_document_context_budgeting_cas_5787`). | quoted lines | "Bug filing: references/filing-cas-bugs.md"; move the budgeting note to a test comment; retarget pins | −140 always |
| 63 | P2 | on-demand | `model-selection.md:18-21,57-100,134,147,155-167`; `reference.md:83-85,91` | 3,8 | ≈4 KB of OpenCode/QwenCloud token-plan detail (09-02 finding, grown) in every supervisor's routing doc; fan-out paragraph duplicated in `reference.md`. | byte spans | `references/opencode-lanes.md`, read only when `cli=opencode` | −1,000 |
| 64 | P2 | on-demand | `worker-recovery.md:36-38,49-55,95,158-176` | 4,8 | Incident narration (ticket IDs, silent-owl-56, kill internals already in `cas factory kill --help`); "Legacy Verification Jail" for binaries older than v2.0.0; raw `sqlite3 … prompt_queue` where `coordination action=message_status` exists. | `agent_search_system/message.rs:2889-2900` | one sentence each; delete legacy section; MCP call | −1,085 |
| 65 | P2 | on-demand | `cas-worker/references/close-gate.md:50,52,54,60-61`; `recovery.md:8,16,18-20,59`; `details.md:41` | 8 | `commit_receipt` explained ×3, `completion_receipt` ×2, never-bypass ×2, no `gh pr --base epic/` ×2, rebase ×2, drain inbox ×3. | spans | close-gate.md is the one full copy; recovery.md points | −700 |
| 66 | P2 | per-invoke | `model-selection.md:23,138-149` | 4,8 | `max` section is citation trivia (1,595 B) and repeats `:23`. | spans | two lines | −450 |
| 67 | P2 | per-invoke | `cas-memory-management/SKILL.md:34-69,84-102,104-115` + refs | 8 | Overlap outcomes, choose-a-record table, frontmatter keys (×4), `bypass_overlap` (×3) restated across SKILL/refs; Request Fields restates the MCP schema (pinned order). | `mcp_action_surface_test.rs:273-289`; `builtin_flavor_drift_test.rs:1124-1150` | SKILL keeps 3-line pointers; trim field lines to non-obvious semantics | −400 |
| 68 | P2 | per-invoke | `cas-supervisor-checklist.md:60-67,71-75,86-99` | 8 | Intake, review, demo and reporting gates restated from `intake.md`, `planning.md`, body `:27,31-33`. | spans (≈1.68 KB) | one-line pointers | −375 |
| 69 | P2 | on-demand | `intake.md:3-7,29-50` | 5,8 | Stance before gate (09-02, still present); restates cas-ideate/cas-brainstorm triggers (1.9 KB). | spans | gate first; keep the 3-line decision tree | −400 |
| 70 | P2 | per-invoke | `verify-before-claim/SKILL.md:9-17,89-97,99-103` | 4,8 | Motivation + v1 "advisory vs required-paste" decision narration + mantra restate the four steps (pinned `verify_before_claim_skill_test.rs:79`); Opus 5 guidance warns against carried-over verify instructions. | spans | delete; fold the worker proof step into close-gate.md as Check 7 | −400 |
| 71 | P2 | on-demand | `reference.md:28,37,40` | 8 | Commander reply rules duplicate always-loaded body `:34-36`. | spans | keep body; reference keeps the `in_reply_to` example | −125 |
| 72 | P2 | on-demand | `workflow.md:42-56` vs `reference.md:86`; `planning.md:35,37,77,81,126` | 8 | Spawn `task_id` rules ×2; planning restates itself and workflow search guidance. | spans | single copy each | −370 |
| 73 | P2 | always (worker) | `cas-worker.md:18,95-99` | 4,8 | Never-self-dispatch rule stated five times, each phrasing pinned (`builtins.rs:7495-7499`). | spans | one sentence, one pin | −45 |
| 74 | P2 | per-invoke | `cas-search.md:51-63`; `cas-task-tracking.md:31,33`; `cas-search.md:67,69`; `cas-memory-management/SKILL.md:17-20` | 4,8 | Decision guide repeats the action section; "Valid Actions" sections carry maintainer instructions ("dispatch order … keep synchronized") shown to agents; server already returns the valid list on an unknown action. Action sets themselves are now exact (09-02 P0 #9 fixed; pinned). | `service/mod.rs:292,394,927` | delete preambles, keep pinned shape | −215 |
| 75 | P2 | on-demand | `cas-supervisor/references/preflight.md:41-45` | 5 | Redaction internals; still no fail branch (09-02). | span | "Nonzero exit → do not spawn; fix the finding named by its code." | −70 |
| 76 | P2 | per-invoke (Stop job) | `learning-reviewer.md:15,27`; `rule-reviewer.md:11,21,51-52` | 8,6 | `skill action=list_all` once per learning (≤ 20); "complete list" vs hook cap 20; rule-reviewer's `list_all` shows 60-char previews and no `last_accessed`, so its "unused 30+ days" criterion is uncheckable. | `session_stop/mod.rs:136,176,318-330`; `rules.rs:735-750` | call list once; process job IDs with `rule action=show`; drop the 30-day criterion | −60 |
| 77 | P2 | per-invoke | `task-verifier.md:163,220,225,243,311` | 6 | Step 7 `search action=search` queries the knowledge base, not the diff (still present); `rg … src/` fails in repos without `src/` (rc=2 read as "zero references" → false dead-code rejection). | `ops_secondary.rs:53-56`; `rg -n x src/` in cas-src → IO error | `git diff <base>..HEAD \| rg -n '^\+.*(TODO\|FIXME\|todo!\|unimplemented!)'`; `git grep -nw <symbol>` | 0 |
| 78 | P2 | on-demand | `worker-recovery.md` whole; `workflow.md` whole | 3 | 19.6 KB / 19.3 KB references (> 100 lines) without a contents list. | rubric Axis 3 | add a 5-line contents list | +60 |
| 79 | P2 | on-demand | `blame` in `cas-search.md:69` | 5 | Valid action never documented. | `ops_secondary.rs:174-196` | one line + a decision row | +40 |
| 80 | P2 | on-demand | `cas-worker/references/close-gate.md:24,151`; `recovery.md:155,212,227-229`; `cas-worker.md:125`; `reminders.md:94-96` | 4 | Dated incident narration and ticket IDs (rubric Axis 6 retired vocabulary). | spans | delete | −205 |
| 81 | P2 | on-demand | `cas-worker/references/details.md:107,114-126` | 6 | Example receipt is `cargo test -p cas` (contradicts no-build); maintainer section pinned by the context-budgeting test and citing a non-repo memory file. | spans | non-Rust example; move maintainer text to the test | −90 |
| 82 | P2 | on-demand | `reporting-and-routing.md:11-13` | 8 | Issue-registry routing stated a fourth time (not pinned). | span | link | −110 |
| 83 | P2 | on-demand | `overlap-detection.md:35,51-52`; `SKILL.md:54-55` (memory) | 6,4 | "Score conservatively" (scoring is automatic); autofix implies an authorisation check that doesn't exist; "remember defaults to project scope" implies other scopes work (`scope` is ignored on remember). | `cas-core/src/memory/overlap.rs:326-410`; `memory.rs:420-431,731` | delete / reword | −10 |
| 84 | P2 | per-invoke | `cas-task-tracking.md:9` | 8 | "instead of TodoWrite" is the third copy (init CLAUDE.md block, project `cas` skill). | 09-02 §CLAUDE.md | keep (on-demand) — record only | 0 |
| 85 | P2 | on-demand | `epic-flow-walk.md:26,70` | 6 | Ledger path hard-coded to `~/.cas/artifacts/…`, only the default of `[factory] artifacts_root`. | `config/settings.rs:786-797` | `<artifacts_root>/<epic-id>/LEDGER.md` | 0 |
| 86 | P2 | per-invoke | `task-verifier.md` emphasis; `learning-reviewer.md:9-11,37,61` | 4 | task-verifier ≈ 14 all-caps emphasis words and 63 bold spans; learning-reviewer CRITICAL/EACH/EVERYTHING and `mark_reviewed` stated three times. (Supervisor/worker bodies: 0 capitalised NEVER/MUST/IMPORTANT — good.) | counts | plain imperatives with reasons | −60 |

### P3 — polish (one line each)

| # | file:line | Fix |
|---|---|---|
| 87 | `cas-supervisor.md:25` | add liveness `budget_aborted` (`service/worker_liveness.rs:24`) |
| 88 | `reference.md:42-46` | coordination-action list sits under "## Supervisor override"; add a heading; transfer takes no `reason` (`service/core.rs:694-716`) |
| 89 | `workflow.md:39,108`; `reference.md:89` | vague "task/spec workflow" → planning.md; dead anchor `#spawn_workers-parameters`; "all four backends" → Claude and Codex |
| 90 | `planning.md:144-146,160`; checklist `:88` | empty H2; `design_notes` is a spec field, task field is `design` |
| 91 | checklist `:45` | "cherry-pick into `develop`" → "later merges will conflict" |
| 92 | `model-selection.md:117-121,198`; `planning.md:162` | mislabelled stock-fallback block; `tier:` labels unaligned and read by nothing |
| 93 | `codex/.../cas-supervisor.md:55` | heading "(Claude supervisor + Codex workers)" shown to a Codex supervisor |
| 94 | `crates/cas-factory/src/routing.rs:1241-1270` | generated worker recipes include a `supervisor` lane spawn (`workflow.md:99-100`, `factory-supervisor.md:36-37`) |
| 95 | checklist `:11,108`; `model-selection.md:23` | internal ticket IDs in prose |
| 96 | `reporting-and-routing.md:1`; `filing-cas-bugs.md:24-29` | H2 without H1; paragraph breaks the list |
| 97 | `epic-driving.md:12-13` | maintainer instructions → test comment |
| 98 | `details.md:95` | "only hold/release are role-gated" — `restart_spawn_queue`, `db_branch_*` too |
| 99 | `recovery.md:27` | "note_type=blocker" on a message → `blocker=true` |
| 100 | `verify-before-claim:66,93` | "step 1 … implement" (step 1 is `mine`); "mechanical layer" misattributed |
| 101 | `close-gate.md:9,36` | "all 6 checks" (there is a Check 0); "runs on localhost" for non-web light tasks |
| 102 | `cas-worker.md:21,22,56` | subjectless fragment (keep pinned substrings, prepend "A successful `start` is"); "reset merged" ambiguous; status `ready` collides with the action |
| 103 | `codex/…/cas-worker.md:47-49`, `recovery.md:101-111` | teach Claude's `ToolSearch` to Codex/Grok (pinned `builtins.rs:7418-7420`) |
| 104 | `task-verifier.md:279,358-361,402-409,429,440,3` | `tests?/` matches `src/latest/`; overlapping confidence bands; impact rating has no field; stray `source_ids`; epic-reason child IDs trip the one-ID spawn guard (`pre_tool.rs:847-851`); description says "Spawned automatically" (the closer spawns it); add a one-line reply format |
| 105 | `duplicate-detector.md:21`; `rule-reviewer.md:52`; `session_stop/mod.rs:307` | rule merges don't carry `source_ids`; leftover "archive" wording |
| 106 | `cas-search.md:13,25,28,40` | BM25 wording; `grep` walks the working tree, not the index; `include_provenance` unmentioned; `include_source` defaults true |
| 107 | `response-shapes.md:71`; `overlap-detection.md:7`; `lifecycle-and-storage.md:33`; `schema.yaml:1-3` | Conflict also `is_error`; handoff skips overlap; `set_tier` maps unknown → working; two-document YAML header |

## Per-file scorecard (rubric axes; ✓ pass · ~ partial · ✗ fail)

| File | Surface | Bytes | 1 FM | 2 Desc | 3 Size | 4 Word | 5 Proc | 6 Acc | 7 Parity | 8 Tok | Worst |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `cas-supervisor.md` | always | 6,766 | ✓ | ✓ | ~ (249 B) | ~ | ✓ | ✗ | ✓ | ~ | P0-18, P0-20 |
| `…/references/workflow.md` | on-demand | 19,299 | ✓ | — | ~ | ~ | ✗ | ✗ | ✓ | ~ | P0-11..15 |
| `…/references/reference.md` | on-demand | 17,437 | ✓ | — | ~ | ~ | ~ | ✗ | ✓ | ~ | P0-16, P0-17 |
| `…/references/worker-recovery.md` | on-demand | 19,570 | ✓ | — | ✗ | ✗ | ✗ | ✗ | ✓ | ✗ | P0-03..05 |
| `…/references/model-selection.md` | on-demand | 17,117 | ✗ | — | ✗ | ~ | ~ | ✗ | ✓ | ✗ | P0-20, P0-21 |
| `…/references/planning.md` | on-demand | 12,686 | ✗ | — | ~ | ✓ | ~ | ✗ | ✓ | ~ | P0-22 |
| `…/references/intake.md` | on-demand | 4,552 | ✓ | — | ~ | ~ | ✗ | ✓ | ✓ | ~ | P1-45 |
| `…/references/preflight.md` | on-demand | 3,457 | ✓ | — | ✓ | ✓ | ✗ | ✓ | ✓ | ~ | P2-75 |
| `…/references/reminders.md` | on-demand | 5,313 | ✓ | — | ✓ | ~ | ✓ | ~ | ✓ | ✓ | P1-41 |
| `…/references/epic-driving.md` | on-demand | 1,739 | ✓ | — | ✓ | ~ | ✓ | ~ | ✓ | ✓ | P1-38 |
| `…/references/epic-flow-walk.md` | on-demand | 3,760 | ✓ | — | ✓ | ✓ | ✓ | ~ | ✓ | ✓ | P1-57 |
| `…/references/filing-cas-bugs.md` | on-demand | 2,903 | ✓ | — | ✓ | ~ | ✓ | ✗ | ✓ | ✓ | P0-18 |
| `…/references/operator-reply.md` | on-demand | 555 | ✓ | — | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| `…/references/reporting-and-routing.md` | on-demand | 1,426 | ✓ | — | ✓ | ~ | ✓ | ✗ | ✓ | ~ | P0-18, P1-47 |
| `cas-supervisor-checklist.md` | per-invoke | 6,487 | ✓ | ✓ | ✓ | ~ | ~ | ✗ | ✓ | ✗ | P0-19 |
| `codex/…/cas-codex-supervisor-checklist.md` | per-invoke | 6,751 | ✓ | ✓ | ✓ | ~ | ~ | ✗ | ✓ (sanctioned twin) | ✗ | P0-19 |
| `codex/agents/factory-supervisor.md` | always (Codex) | 2,186 | ~ | ✗ | ✓ | ~ | ✓ | ~ | n/a | ~ | P1-48 |
| `cas-worker.md` | always | 8,239 | ✓ | ✓ | ✗ (11 B) | ~ | ~ | ✗ | ✓ | ~ | P0-10, P0-24 |
| `…/references/close-gate.md` | on-demand | 16,770 | ✗ | — | ~ | ~ | ✓ | ✗ | ✓ | ~ | P0-09 |
| `…/references/recovery.md` | on-demand | 17,117 | ✓ | — | ✗ | ~ | ~ | ✗ | ✓ | ✗ | P0-25 |
| `…/references/details.md` | on-demand | 8,155 | ✓ | — | ✓ | ~ | ✓ | ✗ | ✓ | ~ | P1-56 |
| `…/references/discipline.md` | on-demand | 2,429 | ✓ | — | ✓ | ~ | ✓ | ✓ | ✓ | ~ | P1-31 |
| `verify-before-claim/SKILL.md` | per-invoke | 5,708 | ✓ | ✓ | ✓ | ✗ | ✓ | ✗ | ✓ | ~ | P0-02 |
| `cas-task-tracking.md` | per-invoke | 1,582 | ✓ | ✓ | ✓ | ~ | ✓ | ✗ | ✓ | ✓ | P0-06 |
| `cas-search.md` | per-invoke | 3,850 | ✓ | ✓ | ✓ | ~ | ✓ | ~ | ✓ | ~ | P1-53 |
| `cas-memory-management/` (SKILL + 5 refs) | per-invoke + on-demand | 18,081 | ✓ | ✓ | ✓ | ~ | ✓ | ✗ | ✓ | ~ | P0-23 |
| `agents/task-verifier.md` | per-invoke | 27,671 | ~ (no tools) | ~ | ✗ | ✗ | ~ | ✗ | ✓ | ✗ | P0-01 |
| `agents/learning-reviewer.md` | per-invoke (Stop) | 3,252 | ✓ | ~ | ✓ | ✗ | ✓ | ✗ | ✓ | ~ | P0-07 |
| `agents/rule-reviewer.md` | per-invoke (Stop) | 3,576 | ✓ | ~ | ✓ | ✓ | ~ | ✗ | ✓ | ~ | P0-08 |
| `agents/duplicate-detector.md` | per-invoke (Stop) | 2,148 | ✓ | ~ | ✓ | ✓ | ✗ | ~ | ✓ | ~ | P1-51 |
| `agents/session-summarizer.md` | per-invoke (Stop) | 1,989 | ✓ | ~ | ✓ | ✓ | ✗ | ✗ | ✓ | ✓ | P1-51 |

Parity (Axis 7): all 43 files have **zero non-mechanical drift** after prefix normalisation;
the only differences are sanctioned (Codex checklist line, Grok hetero heading, Codex-only
checklist and agent). All frontmatter carries top-level `managed_by: cas` (rubric P2, portable
form `metadata: { managed_by: cas }`) — recorded once here rather than per file.

## Prior-review status (2026-09-02, items touching L2 scope)

| Item | Status | Where now |
|---|---|---|
| P0 #1 task-verifier `files=` → `files_reviewed` | **REGRESSED** (premise wrong; now dropped) | L2-P0-01 |
| P0 #2 `rg -E` | FIXED (now `grep -E`) | `task-verifier.md:279` (P3 regex nit) |
| P0 #3 `pending_supervisor_review` | FIXED; replacement wording wrong for `completion_receipt` | L2-P0-09 |
| P0 #4 `bypass_code_review` | FIXED | 0 hits |
| P0 #5 undefined escape hatch | FIXED | body `:17` → `reference.md#supervisor-override` |
| P0 #6 `/epic-spec` | FIXED | pinned absent `factory_ops.rs:11346` |
| P0 #7 two merge procedures | FIXED in workflow; cherry-pick STILL PRESENT | L2-P1-34 |
| P0 #8 codex factory-supervisor untiered spawn / monitoring ban | FIXED | new lane contradiction L2-P0-20 |
| P0 #9 "exact list" omissions | FIXED for task/search/memory (test-pinned); coordination list short | L2-P0-17 |
| P0 #10-#12 memory autofix / fake CLI / `--no-overlap-check` | FIXED | — |
| P0 #13 memory type enums / file-store model | store model FIXED; enum **REGRESSED** | L2-P0-23 |
| P0 #20 dangling close-gate link | FIXED | new broken links L2-P1-29 |
| P0 #21 learning-reviewer IDs | FIXED (hook passes ≤ 20) | new create defect L2-P0-07 |
| P1 #7 factory-supervisor description | STILL PRESENT | L2-P1-48 |
| P1 #8 checklist preflight pointer | FIXED | manual recipe still inline L2-P1-43 |
| P1 #15 cas-search doc_type/scope/tags | STILL PRESENT | L2-P1-53 |
| P1 #16 memory update title/validity | FIXED | — |
| P1 #17 additive-only wording | FIXED | — |
| P1 #19 VERIFICATION JAIL / ast-grep / epic gates | FIXED; replacement section mis-aimed | L2-P1-49 |
| P1 #21 `/tmp` final-gate log | FIXED | — |
| P2 worker guidance over budget | FIXED but 11 B headroom | L2-P1-26 |
| P2 harness-enforced rules as prose | STILL PRESENT | L2-P1-30, -31 |
| P2 supervisor tree duplication (lane table, Terra, OpenCode, ref frontmatter) | STILL PRESENT, grown | L2-P2-61, -63, L2-P1-46 |
| P2 `code-review-queue.md` | FIXED in source; stale installed copy (cross-lane refresh P0) | — |
| P2 dead `*_GUIDE` constants | FIXED (`CHECKLIST_GUIDE` still used by a test) | — |
| P2 stance before procedure (intake, preflight) | STILL PRESENT | L2-P2-69, -75 |
| P2 narration (Phase, v1, Tonight's) | memory/search FIXED; verify-before-claim + reminders STILL PRESENT | L2-P2-70, -80 |
| P2 worker-recovery raw SQL `UPDATE` | FIXED (read-only SELECT remains) | L2-P2-64 |
| P2 worker-recovery bands absolute vs percent; two procedures | STILL PRESENT (now three) | L2-P1-34, -35 |
| P3 body cites `project_session_start_truncation.md` | STILL PRESENT (test-pinned) | L2-P2-62 |
| P3 note_type `question` | FIXED; `platform_proof` now missing | L2-P1-52 |
| P3 model pins (verifier sonnet) | FIXED (`model: inherit`) | — |
| P3 rule-reviewer "Archive" | mostly FIXED | P3-105 |
| P3 "Current year: 2026" / changelog comment | FIXED | — |

## Proposed fix order (for the follow-up epic)

1. **Budget first, one change:** L2-P1-27 (cas-src checklist out of the worker body), -30/-31
   (harness-enforced prose), -62 (supervisor maintainer text) — frees ≈ 1.6 KB worker / ≈ 0.9 KB
   supervisor so every later body fix passes the pins.
2. **Verifier + maintenance agents:** P0-01, -07, -08; P1-49..51, -58, -59; P2-60 (≈ −3.2 k tok
   per verification).
3. **Worker correctness:** P0-02, -09, -10, -24, -25; P1-29 (links), -36, -55, -56.
4. **Supervisor correctness:** P0-03..05, -11..17, -19..22; P1-34, -35, -37..44.
5. **Registry key + tracking/search/memory:** P0-06, -18 (with `issue_intake_directive_test`),
   -23; P1-52..54.
6. **Dedup/trim pass:** remaining P2 in Δ × multiplier order.

Each step touches pinned text: run the full pin list above in one no-fail-fast pass at assembly.
Rust-side companions (L3-owned strings): `claude_supervisor_contract` skill list (P1-32),
worker contracts' unconditional push (P0-10), `ops_secondary.rs:1137` `config_dir` schema text
(P0-16), `report_cas_bug` routing (P0-24), Stop-hook light-lane prefix remap (P1-58), memory
`scope` ignored on remember (P2-83), tool-description action lists omitting `mark_reviewed`,
`request_changes`, `reset`.

## Search manifest

| Command | Hits | Meaning |
|---|---|---|
| `python3 extract_body replica` over `{,codex/,grok/}skills/cas-{supervisor,worker,supervisor-checklist}.md` | 8 files | body bytes in the measurement table |
| `rg -n "files_reviewed=\|files=" agents/task-verifier.md` | 4 `files_reviewed=`, 0 `files=` | P0-01 |
| `sed -n 350,396p crates/cas-mcp/src/types/ops_secondary.rs` | `pub files` | tool struct field |
| `sed -n 4164,4185p …/lifecycle/close_ops.rs` | `initial_state = AwaitingVerification` | P0-09 |
| `sed -n 2778,2786p service/factory_ops.rs` | `limit == 0` → all | P0-03 |
| `sed -n 164,175p crates/cas-store/src/agent_store/ops_task_leases.rs` | owner check | P0-04 |
| `sed -n 128,142p mcp/tools/types/task.rs` | TASK CREATE REJECTED | P0-06 |
| `sed -n 948,970p service/core.rs` | invocation required; scope global; draft false | P0-07 |
| `sed -n 9,13p config/access/mod.rs` | mecha_cassy deprecated | P0-18 |
| `rg -n stranded_branch_override mcp/tools/types/task.rs` | 1 | P0-19 |
| `awk '/^\[lanes/,0' lane-registry.toml` | 5 lanes | P0-20, P1-39, P1-40 |
| `rg -n "Use skills" crates/cas-pty/src/pty.rs ui/factory/app/mod.rs` | 4 | P1-32 |
| `ls ~/.claude-daniel@petrastella.io/skills/cas-supervisor/` | `SKILL.md references` | P1-29 installed layout |
| `rg -n "intake.md\|planning.md" cas-cli/src/builtins` | registrations only | P1-45 |
| `rg -n "escape hatch\|bypass_code_review\|pending_supervisor_review\|/epic-spec\|VERIFICATION JAIL" cas-cli/src/builtins` | 0 | prior P0s fixed |
| `rg -n "Current year" cas-cli/src/builtins/agents` | 0 | prior P3 fixed |
| `rg -n "TASK_TRACKING_GUIDE\|MEMORY_GUIDE\|SEARCH_GUIDE" cas-cli/src` | 0 | prior P2 fixed |
| normalised `diff` of 43 files × 2 twins | 0 non-mechanical | Axis 7 |
| test-fn scan for `cas-{worker,supervisor}.md` / `*_GUIDE` / `*_guidance(` | 48 tests | pinning table |
| `exa-search` (15 queries) | 15 principles | yardstick |

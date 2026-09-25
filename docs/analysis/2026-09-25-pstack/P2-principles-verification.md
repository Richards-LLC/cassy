# P2 — pstack principles and verification skills vs Cassy gates, hooks and QA

Scope: findings only. Sources, read-only, under `~/research/pstack/cursor-plugins/`: the `pstack/skills/principle-*` skills, `create-verification-skill` and `maintain-verification-skill` with the feature-map example, all of `cursor-team-kit`, and `thermos/` and `continual-learning/`. On the Cassy side: task-verifier, the close gates (`qa_evidence.rs`, `close_ops.rs`), cas-qa-craft and journeys, the shipped `visual-qa.mjs` and `terminal-qa.mjs` scripts, PreToolUse hooks, rules, learning-reviewer and `rule action=promote`, fallow, and the WP1 call-shape lint.

## Verdict

- **Proof: Cassy enforces more in code.** Its close gates, bundle validation and hooks enforce what pstack only states as prose "prove it works".
- **Knowing *how* to prove a project: pstack is far ahead.** `create-verification-skill` produces a per-project launch, doctor, drive and cleanup harness with a feature map. Cassy has a generic QA procedure. Its drive tooling (`journey-eval.sh`, `journeys-for-diff.py`) is cas-src-only, and downstream projects get no harness.
- **Learning: Cassy's loop stops at prose.** Lessons end as synced Markdown rules. pstack's own ladder puts prose rules near the bottom, and nothing in Cassy turns a repeated rule into a lint or check.

## Corrections to the brief

- There are **23** `principle-*` skills, not 24. The 24 comes from counting the bare `principle-` string in the README.
- `cursor-team-kit/skills/control-cli` and `control-ui` are **prose only**: each directory holds just `SKILL.md`, with no CLI and no feature map. The "verification skill = CLI in the skill dir + feature map" idea lives in pstack's `create-verification-skill` and its example `control-notes` CLI (`references/feature-map-example/`). `pstack/README.md:234` points control-cli and control-ui at it.

## (a) The 23 principles vs Cassy

Legend:

- **Enforced:** a code gate, hook, test or structure makes it happen.
- **Prose:** a skill or instruction says it.
- **Missing:** neither.

| # | Principle | Cassy | Evidence |
|---|---|---|---|
| 1 | attack-the-premise | Missing | Nothing tells an agent to stop after two failed fixes that share a premise. The closest thing is an operator memory ("no gate loops"), which is not shipped. |
| 2 | boundary-discipline | Prose (partial) | `cas-codebase-design` covers seams. Nothing covers where validation goes. |
| 3 | build-the-lever | Prose (partial) | Levers do ship: `terminal-qa.mjs`, `visual-qa.mjs`, `journeys-for-diff.py`, fallow. But no skill tells agents to build one, and no gate asks for the rerunnable script. |
| 4 | encode-lessons-in-structure | Prose (partial) | The learning → draft rule → promote loop exists, but it ends in prose `.claude/rules`. `hook_command` on rules is retired (`cas-types/src/rule.rs`). The call-shape lint and the skill-text pins are one-off structural encodings. |
| 5 | exhaust-the-design-space | Prose | `cas-brainstorm`, `cas-ui-craft` (concept brief). |
| 6 | experience-first | Enforced (partial) | `qa_evidence.rs` enforces a rubric floor of 4 on distinctiveness, fit and hierarchy, plus a11y modes and light/dark renders. Journey experience is scored 0–3 in prose. |
| 7 | fix-root-causes | Prose | `cas-diagnosing-bugs`; the "Don't assume — always verify" section in the repo `AGENTS.md`. |
| 8 | foundational-thinking | Prose | `cas-codebase-design`. |
| 9 | guard-the-context-window | Enforced | Tests pin budgets: SessionStart `SESSION_START_BUDGET_BYTES` (`builtins.rs`), `tools/list` ≤ 59,000 B (`mcp_action_surface_test`), task-verifier < 15 KB (`agent_definition_contract_test`). A 10,000-char instruction-file cap runs in Docs Lint. |
| 10 | laziness-protocol | Missing | No shipped guidance biases toward deletion or the smallest diff. |
| 11 | make-operations-idempotent | Enforced (by design) | Migrations carry `detect` queries; `update` runs a backup/rollback transaction; Stop jobs use `create_new` queue markers; the `plan_claude_md` and `plan_agents_md` plans are idempotent and tested. |
| 12 | migrate-callers-then-delete-legacy-apis | Deliberately contrary | Cassy keeps one-release aliases (e.g. `mecha_cassy` → `violet`) because downstream projects are external users. pstack's own rule exempts that case. |
| 13 | minimize-reader-load | Prose (docs only) | `cas-writing-for-agents` covers agent-facing text. Nothing covers code. |
| 14 | model-the-domain | Prose | `cas-codebase-design` (domain terminology). |
| 15 | never-block-on-the-human | Enforced (factory) | A PreToolUse hook blocks `AskUserQuestion` for factory agents (`pre_tool.rs`, `config/hooks.rs`), and the worker brief bans foreground waits. Interactive sessions: prose only. |
| 16 | outcome-oriented-execution | Enforced | The epic → lanes → assembly flow is this principle: breakage is allowed inside lanes, and one `ASSEMBLY_PROOF` verifies the end state. |
| 17 | prove-it-works | Enforced | Close gates block completion until a sealed task-verifier verdict exists (`close_ops.rs`). Merge-state and stranded-branch gates, bundle freshness and ledger validation (`qa_evidence.rs`), and terminal-qa/visual-qa receipts do the same for delivery evidence. The `verify-before-claim` skill backs this up. |
| 18 | redesign-from-first-principles | Missing | — |
| 19 | separate-before-serializing-shared-state | Enforced | Isolated worktrees per worker, a separate target dir each, task leases, `lane.lock` for the light lane, per-agent queue markers. |
| 20 | sequence-verifiable-units | Enforced at epic granularity only | Workers may not build, so unit checks move to CI Scoped Validation and assembly. Cost this epic: 7 integration failures at assembly run 1, and two WP8 compile errors that only CI caught. |
| 21 | subtract-before-you-add | Missing | Wave A did it as audit work, but nothing ships it as guidance. |
| 22 | test-behavior-not-implementation | Prose, and contradicted in practice | `cas-tdd` says it. Cassy's own suite leans hard on the "constant pin" shape pstack rejects: skill-text `contains(...)` pins such as `builtins.rs` markers and `agent_definition_contract_test`. Those pins block wording edits (memory "Skill text pins") and would pass on any text that keeps the marker. |
| 23 | type-system-discipline | Missing as guidance | The Rust compiler does part of the work; no skill states the rule. |

Totals: 8 enforced (2 of them only partly or at a coarse grain), 9 prose (one contradicted in practice), 1 deliberately contrary, 5 missing.

## (b) Verification skills and feature map vs Cassy QA

| Concern | pstack (`create-` / `maintain-verification-skill`) | Cassy |
|---|---|---|
| **Per-project drive harness** | Generated `.cursor/skills/verify-<app>/`: Launch with a readiness signal, Doctor (read-only health: process, build, port ownership), Drive with real selectors, Evidence, Cleanup ("kill what you started"; evidence survives), executable Helpers. | None generated. `cas-qa-craft` is a generic procedure. `journey-eval.sh` and `journeys-for-diff.py` exist only in cas-src `scripts/`, so a downstream project learns its launch and drive steps from scratch every pass. |
| **Feature map** | `features/README.md` index plus one file per feature. Each has four fixed H2s: `Sub-features`, `How to get to it (user POV)`, `Driving it with <harness>`, `Gotchas`. Drives are exact commands with observable results. The rule "do not report a skipped entry point as verified through a different path" is in the map. | `docs/qa/journeys.md` holds journeys: Steps, Touches globs, Entry → Goal, edge paths. They are cross-feature flows, not a per-feature catalog of entry points. Nothing requires that *every* entry point of a feature be covered. |
| **Self-proof of the harness** | The generator must run launch → doctor → one drive → cleanup, and confirm the evidence survived, before handing over. | No equivalent. A QA pass trusts whatever setup it improvises. |
| **Maintenance** | A dedicated loop: index hygiene, one read-only source subagent per feature, a mandatory live pass, triage into doc drift / harness gap / product regression. Outcome is `clean`, `changed` or `blocked`, and the loop never edits product code. | `journeys-for-diff.py --check` catches Steps vs `test.step` drift in cas-src only. No drift pass exists for a downstream map. |
| **Evidence at close** | Prose standards only. Nothing blocks. | Enforced: ledger REJECT table, bundle freshness against the delivered head, visual-qa rubric floor, terminal-qa receipt, skip-marker refusal, capture judgment by the verifier (`qa_evidence.rs`, `verifier-evidence-gate.md`). |
| **Claim-level proof** | `verify-this`: restate the claim as testable, baseline vs treatment from the same command, verdict `VERIFIED`, `NOT VERIFIED` or `INCONCLUSIVE`. | `verify-before-claim` skill (prose); task-verifier checks acceptance criteria, not a before/after artifact. |

**Answer:** yes, build a Cassy `create-verification-skill` (recommendation R1 below). Cassy's gates prove *that* evidence exists and is fresh. pstack's generator makes the evidence cheap and repeatable per project. The two fit together: the generated `features/` map should feed the ledger matrix, one entry point per matrix row, and the generated Doctor and Launch should become the `bundle.json` producing command.

## (c) The enforcement ladder applied to Cassy's learning loop

pstack's ladder, strongest mechanism first: an unrepresentable state, then a lint or banned API that fails CI, then a canonical helper, then a runtime check, then rules and skills, then review. Routing: one-off → note; recurring → skill or lint; systemic → principle.

| Rung | Where Cassy lands today | Gap |
|---|---|---|
| Review | task-verifier rejections; the harness `/code-review` (Cassy retired its own review workflow) | Fine. |
| Rules / skills | Stop → learning-reviewer → draft rule → `promote` → `.claude/rules/cas/*.md`. The verifier also writes draft rules on rejection. | **Every lesson terminates here.** Nothing asks whether a lesson that keeps recurring should become a check. |
| Runtime check / hook | Hand-built hooks: cargo guard, `AskUserQuestion` block, push guard, history-rewrite guard, Neon SQL guard. | These appear only when an engineer writes one. Rules lost their automation field (`hook_command` is retired). |
| Lint / CI | The call-shape lint (WP1), the operator-data lint, the size caps, drift tests. | Built by audits, not by the learning loop. |
| Unrepresentable | Enum-typed actions (`cas_mcp::actions`), typed `RuleStatus` and `VerificationStatus`. | — |

Cassy's loop is strong on capture: it never loses a correction. It is weak on routing: nothing ever pushes a lesson above the prose rung. continual-learning is weaker still, writing only `AGENTS.md` bullets. `reflect` and `workflow-from-chats` send lint-shaped items to a backlog.

## Recommendations

Effort: S ≤ ½ day, M ≈ 1–2 days, L ≈ a week.

| ID | Adopt / skip | Recommendation | File(s) | Effort |
|---|---|---|---|---|
| R1 | **Adopt** | New builtin `cas-verification-skill`. It generates `.claude/skills/verify-<app>/` in a downstream project, with Launch, Doctor, Drive, Evidence and Cleanup sections, executable helpers, and a `features/` map in pstack's four-H2 format. It must prove itself once (launch → doctor → one drive → cleanup, evidence survives). Map rows feed the cas-qa-craft matrix, and Doctor/Launch become the bundle's producing command. | `cas-cli/src/builtins/skills/cas-verification-skill/SKILL.md` (+ `references/feature-map-example/`, ×3 flavours, registered in `builtins.rs`); `cas-qa-craft/SKILL.md` step 2 points at the map | M |
| R2 | **Adopt** | A maintenance pass for that map: index hygiene, one source subagent per feature, a mandatory live pass, and `clean`/`changed`/`blocked` outcomes. Wire it as a supervisor chore after epics that touch `qa.user_facing_paths`. | `cas-cli/src/builtins/skills/cas-verification-skill/references/maintain.md`; `cas-supervisor/references/workflow.md` (one line) | S |
| R3 | **Adopt** | Promote the learning loop past prose. rule-reviewer tags a promoted rule `enforceable:<lint\|hook\|type>` when a check can express it. `rule action=promote` then files a `chore` task "encode <rule> as <mechanism>", naming the rule and its source IDs. This is the pstack ladder's routing step. | `cas-cli/src/builtins/jobs/rule-reviewer.md`; `cas-cli/src/mcp/tools/core/rules.rs` (`cas_rule_promote`) | M |
| R4 | **Adopt** | Replace prompt/skill-text constant pins with behavioral checks where a mechanism exists (the audit-era pins in `builtins.rs` and `agent_definition_contract_test`). Keep pins that guard a relation across files, which pstack explicitly allows. Start with the markers that blocked WP2/WP8 wording edits. | `cas-cli/src/builtins.rs` (marker tests), `cas-cli/tests/agent_definition_contract_test.rs` | M |
| R5 | **Adopt** | Ship the missing principles as one compact reference, not 23 skills: attack-the-premise, laziness/subtract-before-you-add, redesign-from-first-principles, test-behavior (with the "passes if every import returns undefined" check), type-system discipline. Load it on demand from `cas-diagnosing-bugs` and `cas-codebase-design`, not at SessionStart. | `cas-cli/src/builtins/skills/cas-codebase-design/references/principles.md`; one link each from `cas-diagnosing-bugs/SKILL.md` and `cas-tdd/SKILL.md` | S |
| R6 | **Adopt** | The attack-the-premise stop rule in code. After a second failed close or CI round on the same gate for one task, the close guidance says to write down the premise and take a census before another fix. This matches the operator's "no gate loops" memory. | `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs` (guidance on a repeated gate refusal) | S |
| R7 | **Adopt** | A `verify-this` claim mode in task-verifier. When acceptance criteria state a measurable claim (perf, size, count), require a baseline/treatment pair from the same command and a `VERIFIED`/`NOT VERIFIED`/`INCONCLUSIVE` line. | `cas-cli/src/builtins/skills/cas-qa-craft/references/verifier-evidence-gate.md`; `agents/task-verifier.md` (Step 0 pointer) | S |
| R8 | **Adopt (small)** | Per-unit verification inside lanes without cargo. Allow `cargo check -p <crate>` (no test, no link) for workers behind the build cache, or run it as an automatic lane pre-merge job. It would have caught both WP8 compile errors before CI. | `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs` (cargo guard allowlist) or `.github/workflows/ci.yml` (Scoped Validation already does this; surface its result to the worker) | S (CI) / M (guard) |
| R9 | Skip | control-cli / control-ui as-is. They are prose snippets, and `cas-playwright-debug`, `terminal-qa.mjs` and `visual-qa.mjs` already cover them with enforced receipts. | — | — |
| R10 | Skip | continual-learning's `AGENTS.md` bullet writer. Cassy's Stop jobs plus session-learn already capture more, with dedup and history. Writing into canonical `AGENTS.md` would fight the D3 projection. | — | — |
| R11 | Skip | thermos / thermo-nuclear review as a Cassy builtin. Review is the weakest rung, and the harness `/code-review` already runs a parallel-review pass. The one idea worth borrowing is the "file must not pass 1,000 lines" hard line, and it only works as a lint (a CI file-size check), not as review prose. | — | — |
| R12 | Skip | migrate-callers-then-delete-legacy-apis for public surfaces. Cassy has external users, so its one-release deprecation window is correct. Apply the principle only to internal Rust APIs. | — | — |

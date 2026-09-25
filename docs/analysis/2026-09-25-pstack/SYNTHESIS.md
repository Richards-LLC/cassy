# pstack vs Cassy: synthesis

Task cas-a498, epic cas-9081. This merges three lane reports (all on epic cas-143c at `b3cc72036`) and the talk transcript:

- `P1-workflows.md`: the workflow skills and playbooks, plus the orchestrate plugin.
- `P2-principles-verification.md`: the 23 principles, the verification skills, and the enforcement ladder.
- `P3-packaging.md`: cross-harness distribution and the learning loops.
- `~/Downloads/2026-09-25-poteto-2500-prs.md`: the talk.

Sources: pstack v0.15.5 (`cursor/plugins` `78f46da`) and the pstack-claude port v0.9.44 (`9f3a2ca`). Findings only; no product code changes.

## Verdict

**Cassy enforces; pstack judges.** Cassy's lead is mechanism:

- close gates and sealed verification verdicts;
- durable tasks, leases and memory;
- pinned context budgets;
- one runtime contract across Claude, Codex and Grok.

pstack has no hooks at all, and its "prove it works" is prose.

pstack's lead is the thinking inside a task:

- multi-model adversarial review;
- reviewable decision trails;
- evidence vocabulary (`VERIFIED` / `NOT VERIFIED` / `INCONCLUSIVE`, a proof ladder);
- a per-project harness that makes verification cheap to repeat;
- an explicit failure policy for fleets.

The talk's thesis, "make the easy path the right path; route every correction to the strongest layer", is already Cassy's operating model. The gap is that Cassy's learning loop still ends at prose rules.

**Adopt pstack's judgment layer on top of Cassy's enforcement, not in place of it.**

- **Adopt now (current epic):** five ranked items are skill or guidance text that fit the current epic. Two more are mostly done by the Wave A–D work already in flight.
- **Adopt next (new epic):** the three highest-value items each need a new skill or a runtime change:
  - a verification-harness generator;
  - multi-model interrogate;
  - routing lessons to lints and hooks.

## Where each side is stronger

Scale: 0 = absent, 1 = prose or partial, 2 = a mechanism in some cases, 3 = enforced or complete.

| Capability | Cassy | pstack | Lead | Source |
|---|---:|---:|---|---|
| Proof enforced before close (gates, sealed verdicts) | 3 | 1 | Cassy | P2 (a) #17 |
| Durable state: tasks, leases, epics, memory store | 3 | 1 | Cassy | P1 Verdict; P3 (b) |
| Context budgets pinned by tests | 3 | 1 | Cassy | P2 (a) #9 |
| Learning capture: store, dedup, history | 3 | 1 | Cassy | P3 (b) |
| One runtime contract across Claude, Codex, Grok | 3 | 2 | Cassy | P1 Verdict; P3 (a) |
| Shared-state isolation (worktrees, leases, locks) | 3 | 2 | Cassy | P2 (a) #19 |
| Multi-model adversarial judgment | 0 | 3 | pstack | P1 Verdict (1); `interrogate`, `arena` |
| Reviewable decision trails | 1 | 3 | pstack | P1 Verdict (2); `show-me-your-work` |
| Claim-level evidence vocabulary | 1 | 3 | pstack | P1 Verdict (3); P2 (b) "Claim-level proof" |
| Per-project verification harness and feature map | 1 | 3 | pstack | P2 (b) |
| Explicit fleet failure policy | 1 | 3 | pstack | P1 Verdict (4); orchestrate |
| One skill tree for every harness | 1 | 3 | pstack (closing: WP12) | P3 (a) |
| Routing lessons above prose (lint, hook, type) | 1 | 2 | pstack | P2 (c); `reflect` step 4 |

## Ranked adopt list

Deduplicated across the three lanes. Rank is by value, then by lower effort.

- **Value:** 5 = changes outcomes on most tasks; 1 = polish.
- **Effort** (P1 scale): S = skill text, under ½ day; M = 1–2 days, skill plus reference or script plus pinned test; L = runtime or Rust work over several days.
- **Fit:** *current* = can ride epic cas-143c as skill or guidance text; *covered* = the Wave A–D work already does it or is doing it; *new* = needs its own epic; *decision* = needs an operator call first.

| # | Adopt | Value | Effort | Cassy change | Fit | Traces to |
|---:|---|---:|---|---|---|---|
| 1 | Verification-harness generator: per-project Launch/Doctor/Drive/Evidence/Cleanup plus a `features/` map; the harness proves itself once, and the map feeds the QA ledger | 5 | M | new `cas-verification-skill` (+ maintain pass); `cas-qa-craft` step 2 | new (D7 already ships the QA scripts) | P2 R1, R2; P1 #10 |
| 2 | `cas-interrogate`: the same review prompt across model families, sorted into Act on / Consider / Noted / Dismissed with an agreement map | 5 | M | new `cas-interrogate` skill; route from `cas-supervisor/references/workflow.md` | new | P1 #2 |
| 3 | Route lessons to structure: rule-reviewer tags `enforceable:<lint\|hook\|type>`, and `promote` files an "encode as mechanism" chore. The learning reviewer asks "could a check enforce this?" before writing prose | 5 | M | `jobs/rule-reviewer.md`, `jobs/learning-reviewer.md`, `mcp/tools/core/rules.rs` | new (builds on D11 `promote`, WP8) | P2 R3; P1 #9; P3 #5 |
| 4 | Proof vocabulary: blast-radius proof ladder plus one safety fact; `verify-this` baseline and treatment from the same command; `VERIFIED` / `NOT VERIFIED` / `INCONCLUSIVE` | 4 | S | `verify-before-claim/SKILL.md`, `agents/task-verifier.md`, `cas-qa-craft/references/verifier-evidence-gate.md` | current | P1 #3, #11; P2 R7 |
| 5 | Fleet failure policy: retry by failure mode; refuse to spawn without brief fields; PASS/ISSUES/BLOCKED report with SHA and method; patch-id re-verify after rebase | 4 | S | `cas-supervisor/references/worker-recovery.md`, `workflow.md`, `cas-worker.md` | current | P1 #4, #5, #6 |
| 6 | `cas-arena`: a multi-model bakeoff with cross-judge, pick and graft, for design forks | 4 | M | new `cas-arena` skill + `builtins.rs` registration | new | P1 #1 |
| 7 | Attack-the-premise stop: after a second refusal on the same gate, write down the premise and take a census before another fix. Ship the missing principles (laziness, subtract, redesign, test-behavior, type discipline) as one on-demand reference | 4 | S | `close_ops/gate_text.rs` (WP6 helpers); `cas-codebase-design/references/principles.md`; links from `cas-tdd`, `cas-diagnosing-bugs` | current | P2 R5, R6; P1 #13, #14, #15 |
| 8 | Finish one tree: deny prefixed tool literals in shared text; collapse the remaining twins with stamped frontmatter or one per-harness reference file | 3 | S+M | `tests/builtin_flavor_drift_test.rs`, `builtins.rs`, `cas-supervisor.md` | covered (WP12a/WP12b in flight) | P3 #1, #2, #3 |
| 9 | Decision trails: `evidence=` pointers in decision notes; an end-of-epic cross-model "Attention" review of the decision notes | 3 | S | `cas-supervisor/references/epic-driving.md`, `cas-worker.md` | current | P1 #7 |
| 10 | Replace constant skill-text pins with behavioral checks, keeping the pins that guard a cross-file relation | 3 | M | `builtins.rs` marker tests, `tests/agent_definition_contract_test.rs` | new | P2 R4 |
| 11 | Session-learn cadence gate (N counted turns and M minutes) plus an incremental transcript index | 3 | M | `hooks/handlers/handlers_session.rs` | new | P3 #4 |
| 12 | `why` workflow: epistemic confidence tiers and a coverage map that records nulls, on top of `search history/blame` | 3 | M | `cas-search.md` or new `cas-why` | new | P1 #8 |
| 13 | Unit checks inside lanes: surface Scoped Validation to the worker, or allow `cargo check -p` (it would have caught both WP8 compile errors before CI) | 3 | S/M | `.github/workflows/ci.yml` or `pre_tool.rs` cargo guard | decision (conflicts with the no-cargo directive) | P2 R8 |
| 14 | Handoff brief contract: capsule ≤ 5, one status tag per thread, problems ≤ 5, one next move | 2 | S | `cas-memory-management/references/body-templates.md` | current | P1 #12; P3 #6 |
| 15 | Unslop rules 7 and 26 as a builtin text lint | 2 | M | `tests/builtin_doc_hygiene_test.rs` | new | P1 #16 |
| 16 | Blinded skill-change eval (needs #6) | 2 | M | `cas-writing-for-agents/SKILL.md` | new | P1 #17 |
| 17 | Vendored-reference sync: pinned SHA, exclude list, 3-way merge and a denylist, first for fallow (audit M54) | 2 | M | new `scripts/` sync + `skills/fallow/` | new | P3 #7 |

**By fit:**

- *current*: #4, #5, #7, #9, #14, all S.
- *covered*: #8.
- *decision*: #13.
- *new*: #1, #2, #3, #6, #10, #11, #12, #15, #16, #17.

## Already covered by the Wave A–D work (cas-143c)

- **D1 one tree with bare tool names (WP12a), and D6/projection (WP12b).** This is pstack's single-tree packaging (P3 (a)), and it makes #8 mostly done.
- **Call-shape lint (WP1).** It checks every suggested `<tool> action=` against the live MCP surface, which is "encode lessons in structure" (P2 (c), Lint / CI rung).
- **D7: `visual-qa.mjs` and `terminal-qa.mjs` ship inside the skills.** This covers the evidence half of the verification harness. #1 adds the per-project launch-and-drive half.
- **D11: a real `rule action=promote` (WP8).** This is the hook #3 needs.
- **Budgets pinned by tests (WP2 SessionStart, WP3 `tools/list`), and the coordination/factory split (WP13).** This is guard-the-context-window (P2 (a) #9).
- **Close-gate helpers (WP6).** They give #7's stop rule one renderer to hang on.

## Skip list

| Skip | Reason | Source |
|---|---|---|
| poteto-mode as a whole | A single-operator style. Its routing duplicates Cassy role guidance; the useful parts are taken item by item above | P1 poteto-mode |
| how, teach, bro | Explainer and persona skills with no gap in Cassy's workflow | P1 |
| automate-me | Preferences already live as typed memory entries; a generated `-mode` skill would drift from them | P1; P3 #9 |
| setup-pstack | Model routing lives in Cassy's lane registry and recipes | P1 |
| no-comments, technical-writing, typescript-best-practices | Covered by harness review and `cas-writing-for-agents`, or stack-specific | P1 |
| make-bot-ui, check-plan.mjs | Cursor or Grok-bot specific | P1 |
| control-cli / control-ui | Prose snippets; `terminal-qa.mjs`, `visual-qa.mjs` and `cas-playwright-debug` already cover them, with receipts | P2 R9 |
| continual-learning's AGENTS.md writer | Fights the canonical managed block (D3). Cassy's store captures more, with dedup | P2 R10; P3 #8 |
| thermo-nuclear review prose | Review is the weakest rung. The one keeper, a file-size ceiling, only works as a CI lint | P2 R11 |
| migrate-callers-then-delete for public surfaces | Cassy has external users, so one-release aliases are correct (as in WP13) | P2 R12 |
| Marketplace plugin as the install path | Cannot register `cas serve`, the hooks or the managed block | P3 #10 |
| Generated Codex slash stubs | A second invocation surface to keep in sync, for no Cassy gain | P3 #11 |

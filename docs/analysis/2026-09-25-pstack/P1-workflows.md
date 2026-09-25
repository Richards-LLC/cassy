# P1: pstack workflow skills vs Cassy

Task cas-f66b, epic cas-9081. Findings only. Source: pstack v0.15.5, read-only at
`~/research/pstack/cursor-plugins/pstack/` (paths below are relative to it unless
they start with `cas-cli/` or `docs/`), the sibling `orchestrate` plugin at
`~/research/pstack/cursor-plugins/orchestrate/`, and the talk transcript
`~/Downloads/2026-09-25-poteto-2500-prs.md`. Cassy side: the builtins under
`cas-cli/src/builtins/` at `379c8f277`.

## Verdict

pstack and Cassy solve different halves of the same problem. pstack is a
**single-operator toolkit of judgment workflows**: most skills fan out to two or
three model families, then one lead synthesizes and records why. Cassy is a
**factory runtime**: durable tasks, leases, epics, merge gates, PreToolUse
guards, verification dispatch and a memory store, with comparatively thin
guidance on *how to think* inside a task.

Cassy is ahead on enforcement, as pstack has no hooks at all (`.cursor-plugin/plugin.json` declares only
`skills` and `agents`). It is also ahead on durable state and cross-harness
parity. pstack is ahead on four things Cassy lacks almost entirely:

1. **Multi-model adversarial judgment**: `interrogate`, `arena`, `architect`, the `eval` playbook.
2. **Reviewable decision trails**: `show-me-your-work`, a TSV log that is audited against the transcript and then cross-model reviewed.
3. **Evidence epistemics**: `why` confidence tiers, the `blast-radius` proof ladder, and VERIFIED / NOT VERIFIED / INCONCLUSIVE verdicts.
4. **Explicit, mechanical failure policy for fleets**: the orchestrate retry-by-failure-mode table, refuse-to-spawn briefs, and the patch-id re-verify rule.

The talk's own thesis ("make the easy path the right path", route every
correction to the strongest layer: codebase, then lint/CI, then skills, then
style guide; transcript l.43, l.89-91) is already Cassy's operating model. Its
pinned tests, call-shape lint, operator-data lint and PreToolUse denials are
that thesis in practice.

Recommendations are ranked at the end. Effort scale: **S** means skill text
only, under half a day. **M** means one to two days: a skill, a reference or
script, and a pinned test. **L** means runtime or Rust work across several
days.

## Mapping, skill by skill

Each entry covers what the skill does and how, the closest Cassy equivalent,
what each side does better, and a recommendation.

### poteto-mode (`skills/poteto-mode/SKILL.md`, 23 playbooks)

- **What it does.** A user-invoked personal "mode" and router. Non-negotiable
  triggers map situations to skills: a nontrivial change goes to `how`, a
  function-boundary change to `architect`, a contested design to
  `interrogate`, prose to `unslop`, and a PR-status request to the Babysit
  playbook. It has a 24-principle index (the `principle-*` skills), an
  autonomy policy ("Just do it"; always pause for irreversible writes), and
  subagent defaults: background, file pointers not inlined context, an
  explicit model per role (code on `grok-4.7-xhigh-fast`, judgment and prose
  on `claude-opus-5-5-max`). Twenty-three playbooks are copied verbatim into
  the todo list. The `reminder:` frontmatter nudges invocation.
  `agents/poteto-agent.md` forces subagents to read the mode first ("Substituting
  `generalPurpose` skips that read and drifts", l.3).
- **Cassy equivalent.** Split three ways:
  - the factory role skills, `cas-cli/src/builtins/skills/cas-supervisor.md` and `cas-worker.md` (the "mode");
  - the CLAUDE.md/AGENTS.md managed block (the always-on reminder);
  - per-task methodology skills (`cas-tdd`, `cas-diagnosing-bugs`, `cas-codebase-design`).

  Cassy has no playbook router.
- **pstack better.**
  - One entry point routes to the right workflow by situation.
  - A step you skip stays in the todo list as `skip: <reason>` (SKILL.md "Playbooks"), so skipped rigor is visible.
  - Subagent model choice is per *role*, not only per worker lane.
- **Cassy better.**
  - Roles are enforced, not requested: the PreToolUse cargo denial for workers, the close gates and verification dispatch.
  - Guidance survives across harnesses (Claude, Codex, Grok) and sessions.
  - pstack's routing depends on the model obeying prose.
- **Recommendation. Skip the wholesale mode.** Adopt two pieces:
  1. The `skip: <reason>` rule for mandated steps. Add it to `cas-worker.md` step list and `cas-supervisor/references/epic-driving.md`. Effort **S**.
  2. Per-role subagent models. Defer it to the lane registry (see setup-pstack).

### architect (`skills/architect/SKILL.md`, `references/{runner-prompt,rationale-template,design-red-flags}.md`)

- **What it does.** Design before code, in five phases.
  - **Ground:** run `how`, plus `why` if ownership changes.
  - **Sketch:** run `arena` on 3 model families, with "Design it twice", meaning at least two structurally distinct whole-shape candidates.
  - **Red-flag screen:** reject shallow modules, information leakage, temporal decomposition and pass-through methods.
  - **Agree:** a human checkpoint, opt-in only.
  - **Implement** against the `not implemented` sketch.
  - **Scrap:** throw the sketch out when a *pattern* of friction appears. Its tells: the same workaround in unrelated places, escape-hatch types, "we need a lock" on unshared state, and two independent deviations of the same shape.
- **Cassy equivalent.** `cas-codebase-design/SKILL.md` "Design it twice" (explore minimum surface, maximum flexibility and the common caller), "Deepening an existing shallow module", and a critique rubric.
- **pstack better.**
  - Candidates come from different models, not one model imagining three.
  - The rationale template records the synthesis decision.
  - It has explicit scrap tells and a scrap procedure (re-ground, subtract, re-run).
- **Cassy better.** Vocabulary depth (seams, locality, leverage) and the API/DX taste rubric.
- **Recommendation. Adopt the scrap tells and the red-flag screen** into `cas-codebase-design/SKILL.md` as two short sections. Effort **S**. The multi-model sketch waits on the arena recommendation.

### arena (`skills/arena/SKILL.md`)

- **What it does.** Six phases:
  - **Frame:** state the artifact and a 3-6 criterion rubric that only the picker sees.
  - **Fan out:** N background runners on different model families, each in its own worktree, each writing an artifact plus rationale.
  - **Cross-judge:** one read-only judge from a different family than the parent.
  - **Pick:** a base, criterion by criterion. The tiebreak is which candidate a future maintainer can extend most easily.
  - **Graft:** port the best one or two ideas from each loser, by hand.
  - **Verify.**

  Convergence of all N means ship the consensus. Wild divergence means Phase A was under-specified, so reframe rather than average.
- **Cassy equivalent.** None as a skill. The factory can spawn N workers (`coordination action=spawn_workers` with `lane=`), and `cas-codex-exec` gives a one-shot Codex opinion, but nothing frames, judges, picks and grafts.
- **pstack better.** Everything here.
- **Cassy better.** Isolation and lifecycle are real: each factory worker gets its own worktree and branch, and delivery is receipt-bound.
- **Recommendation. Adopt** as `cas-cli/src/builtins/skills/cas-arena/SKILL.md` (3 flavours plus registration).
  - Runners: a factory lane per family (`light`/`standard` Codex, `taste` Claude) or `cas-codex-exec` for read-only design packages.
  - The judge is a different lane from the supervisor's.
  - The synthesis note goes in a task note of type `decision`.

  Effort **M**. It unlocks architect, blast-radius step 6 and eval.

### interrogate (`skills/interrogate/SKILL.md`, `references/{reviewer-prompt,rubric,code-quality-review,lead-judgment}.md`)

- **What it does.**
  - One read-only reviewer per model family gets the *same* prompt, rubric and code-quality lens. "The adversarial signal comes from model diversity, not assigned personas."
  - The lead dedupes the findings, maps agreement, and buckets each one as Act on, Consider, Noted or Dismissed, with the models that raised it and a one-line rationale.
  - It never auto-applies changes.
- **Cassy equivalent.** Partial:
  - `task-verifier` (`cas-cli/src/builtins/agents/task-verifier.md`) is a single-model close verifier.
  - `cas-qa-craft/references/independent-pass.md` is an independent QA round.
  - `cas-codex-exec` can give a second opinion.
  - Nothing runs the same review across model families and synthesizes it. (The retired `cas-code-review` persona workflow was the closest; `cas-cli/src/builtins.rs:3462` still prunes it.)
- **pstack better.**
  - Model diversity instead of personas.
  - An agreement map as a signal.
  - Explicit dismiss-with-reason.
- **Cassy better.** The verifier is authority-bound: a sealed dispatch with proof boundaries. Its verdict gates close, while interrogate's verdict is advice.
- **Recommendation. Adopt** as `cas-cli/src/builtins/skills/cas-interrogate/SKILL.md`.
  - Reviewers: a Claude subagent, `cas-codex-exec` read-only, and a Grok lane when available.
  - Output format as pstack's.
  - Route it from `cas-supervisor/references/workflow.md` at the review step for blast-radius-risk tasks. It must not replace the task-verifier gate.

  Effort **M**.

### how (`skills/how/SKILL.md`, `references/{explorer,explainer}-prompt.md`)

- **What it does.** Rates the question simple or complex. Complex questions get 2-4 parallel read-only explorers on the fast code model, then one explainer on the judgment model. Output sections: Overview, Key Concepts, How It Works, Where Things Live, Gotchas.
- **Cassy equivalent.**
  - `cas-search.md` for code search and symbols.
  - The `knowledge` repo wiki (`mcp__cas__knowledge`).
  - `codemap` (`.claude/CODEMAP.md`) and `project-overview`.
- **pstack better.** It produces a task-scoped explanation on demand, with a fixed output shape.
- **Cassy better.** The knowledge persists: codemap and knowledge pages survive the session, while `how` re-derives each time.
- **Recommendation. Skip as a skill.** Adopt the five-section explanation shape as the output contract for `cas-search.md` "explain" answers only if a caller asks for it. Low value; not worth an effort slot.

### why (`skills/why/SKILL.md`, `references/{epistemics,investigator-prompt,synthesizer-prompt,source-playbook}.md`, `references/sources/*`)

- **What it does.**
  - Builds a code anchor (blame, `git log --follow`, PR numbers).
  - Discovers the available MCPs and maps each to one of seven evidence categories. One investigator per category runs in parallel. A category with no MCP is recorded as a gap, never silently skipped.
  - The synthesizer follows `references/epistemics.md` confidence tiers (Direct, Supported, and so on), and the parent must not rewrite the confidence language.
  - Output separates found / inferred / competing hypotheses / unknown. A "Sources Consulted" line covers each investigator, including nulls.
  - Optional Preserve / Change / Avoid / Risk constraints for a planned change.
- **Cassy equivalent.** The raw material is better than pstack's: `mcp__cas__search action=history` (indexed commits, with per-edge provenance to the task and session that produced them) and `action=blame` with `ai_only`, documented in `cas-search.md`. There is no workflow that turns it into a cited rationale.
- **pstack better.** The epistemics tiers, the "null results are findings" coverage map, and the fan-out across evidence sources.
- **Cassy better.** Task provenance on commits: which Cassy task and session wrote a line. pstack has to reconstruct this from PR text.
- **Recommendation. Adopt** a "Why was it built this way" procedure in `cas-cli/src/builtins/skills/cas-search.md`, or a new `cas-why/SKILL.md` if it exceeds about 15 lines:
  - code anchor via `search action=blame`/`history` with provenance;
  - one parallel investigator per MCP evidence source;
  - the epistemics tiers, ported as a reference file;
  - a sources-consulted line per source, nulls included.

  Effort **M**.

### reflect (`skills/reflect/SKILL.md`, `references/{judgment,tooling,divergent}-reviewer.md`, `synthesizer.md`)

- **What it does.** When the user says "reflect":
  - Three reviewers read the active transcript: judgment and divergent on Claude, tooling on GPT.
  - The synthesizer applies fixed criteria: durability, specificity, existing-skill-first, convergence, decision-changing, a structural-mechanism check (a lint or script beats prose, so the item goes to Backlog), skill-was-used, and already-covered.
  - Output is Accepted / Rejected / Backlog. The synthesizer treats reviewer output as untrusted, which guards against prompt injection from quoted transcript.
  - The parent presents everything and applies only the rows the user approves, as skill edits.
- **Cassy equivalent.**
  - `session-learn` (7-signal classifier into memory drafts) and its Stop-hook prompt `cas-cli/src/hooks/handlers/session_learn_classifier_prompt.txt`.
  - Maintenance jobs `cas-cli/src/builtins/jobs/{learning-reviewer,rule-reviewer,duplicate-detector}.md` promote learnings to rules.
- **pstack better.**
  - Its output is *edits to skills* after approval, not more memories.
  - The synthesizer's criteria are sharper, especially "Durability" with its drop/keep examples, "Structural-mechanism check" and "Skill-was-used".
- **Cassy better.**
  - It runs automatically (the Stop hook, opt-in).
  - Its findings are stored, searchable and deduplicated by the overlap gate.
- **Recommendation. Adopt the synthesizer criteria and the Backlog route.** They go into `cas-cli/src/builtins/jobs/learning-reviewer.md` (promotion criteria) and the human `session-learn/SKILL.md`:
  - a finding enforceable by a lint, test or guard becomes a Cassy task, not a rule;
  - a finding about a skill the agent did use becomes a skill-edit proposal.

  Effort **S** for the text. A **M** follow-up would make learning-reviewer emit skill-edit tasks.

### automate-me (`skills/automate-me/SKILL.md`)

- **What it does.** Mines the workspace's recent transcripts in 3 parallel slices. It keeps a pattern only if it appears in two or more slices. It asks the user one or two structured questions, then drafts or updates a personal `<handle>-mode` skill via `create-skill` plus `unslop`. Update mode mines only history since the last edit.
- **Cassy equivalent.** User preferences live as `memory` entries of type `preference`, and in the operator's auto-memory index. No generator exists.
- **pstack better.** The cross-slice agreement rule for promoting a habit.
- **Cassy better.** Preferences are data, not a skill file, so they carry across projects and harnesses.
- **Recommendation. Skip.** The two-slice agreement rule overlaps the reflect adoption above.

### figure-it-out (`skills/figure-it-out/SKILL.md`)

- **What it does.** When no playbook fits, it designs one:
  - **Frame:** a falsifiable done predicate, quantified scope, and a rigor level that is "gates and artifacts, not try harder".
  - **Design:** riskiest unknown first, the verification harness and baseline before the work, and fan out only across seams.
  - **Loop:** hypothesis, smallest change, measure, keep or revert.
  - **Verdicts:** VERIFIED, NOT VERIFIED or INCONCLUSIVE ("Inconclusive is not a pass").
  - "When something passes too easily, suspect the observation method."
  - A show-me-your-work trail.
- **Cassy equivalent.**
  - `cas-supervisor/references/planning.md` and `intake.md` (epic planning) and `cas-brainstorm` (requirements).
  - `cas-qa-craft` uses `NOT EXERCISED`.
  - Cassy has no three-valued run verdict and no "harness before work" rule.
- **pstack better.** The run-level scientific loop and the three-valued verdict.
- **Cassy better.** Plans become durable task graphs with dependencies and gates.
- **Recommendation. Adopt** the falsifiable predicate, the baseline-before-work rule, the three verdicts and the "passes too easily" warning into `cas-supervisor/references/planning.md`. Effort **S**.

### show-me-your-work (`skills/show-me-your-work/SKILL.md`, `references/decision-log-template.tsv`, `scripts/log.sh`)

- **What it does.**
  - One append-only TSV with columns `ts phase decision why evidence result`.
  - `log.sh` stamps rows, strips tabs and newlines, and quote-prefixes `= + - @` against spreadsheet formula injection.
  - A `start` row marks each new run that appends.
  - At the end it audits the log against the transcript: it supersedes wrong rows and never edits them.
  - A **different-family** reviewer reads the trail plus transcript. Every reply ends with an "Attention" section led by `reviewed by <model>`.
- **Cassy equivalent.** Task notes with `note_type=decision|progress|discovery` (`cas-worker.md` l.28, l.77, l.100) and supervisor epic notes (for example `ASSEMBLY_PROOF` rows).
- **pstack better.**
  - One scannable table per run.
  - Evidence is a pointer, never prose.
  - The truth-audit against the transcript.
  - The cross-model "Attention" review.
- **Cassy better.** Notes are durable, searchable and attached to the task, so a reviewer finds them without knowing a file path.
- **Recommendation. Adopt the audit and the Attention review, not the TSV.**
  - For unattended epics, `cas-supervisor/references/epic-driving.md` gains an end-of-epic step: a different-family agent (via `cas-codex-exec` when the supervisor is Claude) reads the epic's decision notes plus task close reasons and returns flagged rows. The result is recorded as an epic note headed `reviewed by <model>`.
  - `cas-worker.md` requires `evidence=` pointers (SHA, file:line, artifact path) in decision notes.

  Effort **S** for the text. A **M** option adds `task action=notes` rendering as a table.

### swarm (`skills/swarm/SKILL.md`)

- **What it does.**
  - Frame states the done predicate and the shape: partition, race or mixed. A race declares `first pass`, `rank all` or `best-of` *before* spawning.
  - Workers are cloud, background and self-contained. They report `PASS`, `ISSUES` or `BLOCKED`, and must list every provable issue.
  - A result that omits the SHAs and method its brief named is dropped and rerun once. A second miss is a gap, and a gap is never a pass.
- **Cassy equivalent.**
  - `coordination action=spawn_workers` plus epic child tasks.
  - `sweep_tasks` (one fix task per integration failure class).
  - The harness `Workflow` tool.
  - No brief/report contract.
- **pstack better.** The declared selection rule and the "SHA plus method or rerun" discipline.
- **Cassy better.** Fan-out is durable and lease-tracked, and workers survive the parent's turn.
- **Recommendation. Adopt** the report contract (PASS/ISSUES/BLOCKED with evidence; name SHA and method; a missing result is a gap) in `cas-supervisor/references/workflow.md` worker briefs and `cas-worker.md` return contract. Effort **S**.

### recall (`skills/recall/SKILL.md`)

- **What it does.**
  - Locks the scope first (a 7-day window, this workspace, stated back).
  - Parallel cheap subagents mine transcript slices.
  - Always sweeps the shared record through `why` investigators when a topic is named.
  - Verifies PRs and branches live.
- **Output contract.** A capsule of at most 5 bullets, one status-tagged line per thread (`[merged #N]`, `[open PR #N]`, `[in flight <branch>]`, `[verified, uncommitted]`, `[reverted #N]`, `[planned, not started]`), at most 5 problems including reverted fixes, and one next move.
- **Cassy equivalent.**
  - `memory` `entry_type=handoff`, injected at SessionStart.
  - `search action=context`.
  - `cas-memory-management/references/body-templates.md`.
  - The supervisor checklist.
- **pstack better.** The status-tag grammar, and "include the fix that shipped and was reverted, so the next attempt starts where the last one failed".
- **Cassy better.** The handoff arrives automatically at the next session, with no mining needed.
- **Recommendation. Adopt** the thread status tags and the problems/next-move shape as the handoff template in `cas-memory-management/references/body-templates.md`. Effort **S**.

### teach (`skills/teach/SKILL.md`)

- **What it does.** Composes `how` and `why` into a paced explanation. It builds diagrams incrementally (redraw and add one part), keeps `why`'s confidence hedges, and uses no framing labels.
- **Cassy equivalent.** None. `cas-html-reports` covers durable reports, not live teaching.
- **Recommendation. Skip.** It is operator-facing polish and has no factory role. Revisit if the why adoption lands.

### tdd (`skills/tdd/SKILL.md`)

- **What it does.** Bug-fix TDD only when a cheap local test path exists. Its stated preference is "Prefer no new test over a bad test", with a list of what makes a test bad. The final report names the failing-before and passing-after evidence.
- **Cassy equivalent.** `cas-tdd/SKILL.md`: seams, vertical slices, behaviour-over-implementation, mocking at boundaries, and the worker no-cargo carve-out.
- **Cassy better.** Broader, and seam-aware.
- **pstack better.** The explicit skip conditions and the "bad test" definition.
- **Recommendation. Adopt** the skip conditions and the bad-test list as a short "When not to add a test" section in `cas-tdd/SKILL.md`. Effort **S**.

### bro (`skills/bro/SKILL.md`)

- **What it does.** Restates the last message in plain language. It is one sentence long.
- **Recommendation. Skip.** No Cassy equivalent, and not needed: Cassy's pane-output rules already cover it.

### setup-pstack (`skills/setup-pstack/SKILL.md`)

- **What it does.**
  - Detects the model slugs usable in this session and asks for a budget tier.
  - Rewrites effort tokens and confirms per role.
  - Writes an always-applied rule, `~/.cursor/rules/pstack-models.mdc`, with one line per role. `inherit-parent`/`auto` mean the role runs on the parent model.
  - "Never write a real slug you have not confirmed is available."
- **Cassy equivalent.** The lane registry (`cas-supervisor/references/model-selection.md`, generated table: `light`, `standard`, `taste`, `heavy`, with fallbacks) and `spawn_workers lane=`.
- **Cassy better.** Registry-driven with loud fallbacks, and shared by every harness.
- **pstack better.** Roles are finer-grained (explorer, explainer, judge, reviewer panel) and user-tunable by budget.
- **Recommendation. Skip.** Adding roles is only worth it once arena and interrogate exist. They can name lanes.

### blast-radius (`skills/blast-radius/SKILL.md`)

- **What it does.**
  - Finds what a change breaks beyond the diff by finding the one fact the change is safe because of.
  - Grades that fact on a 5-rung ladder: said so, pointed at the line, walked the failure, **ran it**, reproduced in the app.
  - Looks where grep stops: the library source at its pinned version, timing, wire formats and flags.
  - Returns confirmed risks and cleared risks separately, and marks "unproven" when needed.
- **Cassy equivalent.**
  - `verify-before-claim` (run the proof fresh).
  - Task `risk=blast-radius` requiring `proof_targets`.
  - Nothing names the safety fact or grades the evidence.
- **pstack better.** The proof ladder and the one-fact focus.
- **Cassy better.** Blast radius is a typed task field with mandatory proof targets checked at close.
- **Recommendation. Adopt** the proof ladder and the "one safety fact, proven or marked unproven" requirement in `verify-before-claim/SKILL.md` for `risk=blast-radius` tasks. Also require the rung in the task-verifier's evidence (`cas-cli/src/builtins/agents/task-verifier.md`). Effort **S**.

### unslop (`skills/unslop/SKILL.md`)

- **What it does.** A numbered anti-pattern list: AI vocabulary, "serves as", em dashes, mid-sentence colons, inline-header lists, abstract metaphor nouns, over-compression and more. Rule numbers are stable ids that other skills cite.
- **Cassy equivalent.**
  - `cas-writing-for-agents` "Wording for current models" (rewritten in WP11).
  - The release-note announce lint (`scripts/release-train-announce.py`).
  - Operator house style lives outside the builtins.
- **pstack better.** Stable rule ids and a concrete replacement for each rule.
- **Cassy better.** Some of the style is mechanically linted (announce lint), which is the talk's own preference.
- **Recommendation. Adopt narrowly.** A small builtin text lint in `cas-cli/tests/builtin_doc_hygiene_test.rs` would flag unslop rule 7 (AI vocabulary) and rule 26 (metaphor nouns) in shipped skill text, with an allowlist. Keep the prose rules in `cas-writing-for-agents`. Effort **M**.

### no-comments (`skills/no-comments/SKILL.md`, `agents/comment-sicko.md`)

- **What it does.** A report-only subagent deletes every comment outside a fixed keep-list ("When I am not sure a keep clause applies, the comment dies", l.21). It flags surprising code for reshape (`MUST KILL`), and offers to encode constraint comments as a type, test or lint. The talk's rationale: agents used comments "as justification for why it wasn't going to solve the actual problem" (l.59-61).
- **Cassy equivalent.** None. cas-src's Rust carries many ticket-id comments.
- **Recommendation. Skip as a builtin.** Comment policy is per project. The constraint-comment-to-test idea is already how cas-src works (pinned tests). File it as a cas-src hygiene idea for P3 or the codebase lane, not here.

### technical-writing (`skills/technical-writing/SKILL.md`)

- **What it does.** A layered doc standard: a Diátaxis mode pick (tutorial, how-to, reference, explanation; "one document, one mode"), Google developer-style sentences, STE instruction rules, and Global English.
- **Cassy equivalent.** `cas-html-reports` (report type times audience taxonomy) and `cas-writing-for-agents` (agent-facing docs).
- **Recommendation. Skip.** Human-facing product docs are outside Cassy's builtins. The Diátaxis "one mode per document" rule could be a line in `cas-html-reports/references/report-types.md` if reports start mixing modes. Nothing observed yet.

### typescript-best-practices (`skills/typescript-best-practices/SKILL.md`)

- **What it does.** Stack rules keyed by the `paths: ["**/*.ts", "**/*.tsx"]` frontmatter.
- **Recommendation. Skip.** Stack-specific. Worth noting: `paths:` is a portable way to scope a skill to file types. The WP11 frontmatter matrix already lists it as honoured by Claude and Grok.

### make-bot-ui (`skills/make-bot-ui/SKILL.md`)

- **What it does.** Builds a local page that POSTs to a Grok Bot webhook. The sender key goes in via a secret-request card, never through chat.
- **Recommendation. Skip.** It is Cursor/Grok-Bot product-specific. Cassy's `cas-wizard` and `mcp-integration` already carry the "credentials never through chat" rule.

### create-verification-skill and maintain-verification-skill

- **What they do.**
  - **create-verification-skill** interviews the *repo* (surface, run, drive, observe, isolate). It generates a project-local `verify-<app>` skill with Launch, Doctor, Drive, Evidence, Cleanup and Helpers sections, plus a `features/` map (one file per user-facing feature: how to reach it, how to drive it, the end state that proves it). It then runs the generated skill end to end once: "A generated skill that was never executed is a draft."
  - **maintain-verification-skill** keeps it honest. One read-only source reader runs per feature file, then one live pass drives every feature. Outcomes are clean, changed or blocked, with at most one PR of proven corrections. It never edits product code.

  The talk calls the feature map "materialized memory", kept current by an automation (l.29-31).
- **Cassy equivalent.**
  - `cas-qa-craft` (a demo matrix per task, the Playwright evidence bundle, `docs/qa/journeys.md` for user journeys).
  - `cas-playwright-debug`.
  - The shipped `cas-ui-craft/scripts/visual-qa.mjs` and `cas-cli-craft/scripts/terminal-qa.mjs` (WP10, D7).
  - No per-project generated harness and no feature map.
- **pstack better.**
  - A durable, project-specific *how to drive this app* skill.
  - A doctor check before driving.
  - "Kill what you started, never by name."
  - A maintenance loop.
- **Cassy better.** Gates: the qa evidence gate refuses a close without the bundle, and independent QA rounds.
- **Recommendation. Adopt**, as a supervisor-owned extension of cas-qa-craft rather than two new skills:
  - `cas-qa-craft/references/verify-harness.md` generates `docs/qa/verify.md` plus `docs/qa/features/*.md` per project, using the same section contract.
  - `docs/qa/journeys.md` then points at feature files.
  - The maintenance pass becomes a periodic supervisor task.

  Effort **M** for the text and template. **L** if the harness scripts are generated and pinned by a test.

## Poteto-mode playbooks and the orchestrate plugin: parts worth taking

The playbooks are mostly compositions of the skills above. The distinctive,
reusable rules are these.

| Source | Rule | Cassy home | Effort |
|---|---|---|---|
| `playbooks/shipping.md` l.9 | **Patch-id rule.** Record verdict SHA, base SHA and `git patch-id`. After a rebase, re-verify only if the patch-id changed. "CI green is not a verdict" (l.7). | `cas-supervisor/references/workflow.md` merge step. Epic merges currently re-run the whole assembly; the patch-id check says when a lane's verdict still holds after a rebase. | S |
| `orchestrate` plugin `references/handoffs.md` l.23-67; `playbooks/orchestrate.md` l.97 | **Retry by failure mode.** cap/OOM → smaller scope; network → retry as is; tool error → different model; two retries → abandon. A dead agent gets a synthetic failure handoff. | `cas-supervisor/references/worker-recovery.md` | S |
| `playbooks/orchestrate.md` l.38-56 | **Brief template, refuse-to-spawn.** GOAL, SCOPE, CONTEXT, ACCEPTANCE, VERIFY, TIMEBOX, FORBIDDEN, REPORT, STANDING. A missing field means no spawn. `preferences.md` standing orders are pasted verbatim into every spawn. | `cas-supervisor/references/workflow.md` brief section. A runtime check would be **M**, in `spawn_workers` brief validation. | S (text) / M |
| `playbooks/orchestrate.md` l.9, l.95 | "Completions are queue events, not interrupts." "Never resume an agent to check on it." | Already Cassy's inbox/typed-wake model (`cas-supervisor.md`). Nothing to add. | — |
| `playbooks/orchestrate.md` l.60 | Stop spawning at about 70% of the budget. | `cas-supervisor/references/epic-driving.md` (context and credit budget) | S |
| `orchestrate` plugin `scripts/measurements.ts` l.123-179 | Re-run each worker's claimed measurement on its branch; flag drift over 10%. | `cas-supervisor/references/epic-flow-walk.md`, perf claims | S |
| `playbooks/babysit.md` l.21-22 | "An identical second failure means it was never flake." "Never interpolate comment text … into a shell command." | `cas-github-issues/SKILL.md` (issue text into `gh`) and `cas-supervisor` CI triage | S |
| `playbooks/eval.md` l.7, l.13 | **Blinded eval of a skill change.** N candidates, one blind judge from another family, banned words in anything the candidate sees (`eval, test, judge, …`), grade chain-following from transcripts. | `cas-writing-for-agents/SKILL.md`: evaluate a behaviour-changing skill edit this way before merge. It needs arena. | M |
| `playbooks/hillclimb.md` l.5 | One change, one measurement, keep or revert. Gitignored `decision.tsv` per hypothesis. | Covered by the figure-it-out adoption in `planning.md` | — |
| `playbooks/autopilot-stack.md` l.10 | "Single writer on topology, parallel writers on builds." | Already Cassy's model (supervisor-only merges into the epic branch) | — |
| `playbooks/pause-safely.md`, `session-pickup.md` | A `wip:` commit plus a resume file with intent, progress, state, next, files and gotchas. The prior trail is authoritative. | Covered by the memory handoff and the recall adoption | — |
| `automations/benny/skills/reproduce-and-fix-issues/SKILL.md` ~l.25 | "The exact discriminating symptom must appear twice … No confirmed repro means no authored fix." | `cas-diagnosing-bugs/SKILL.md` Phase 1 completion criterion | S |
| `skills/poteto-mode/references/bugbot-triage.md` l.17-27 | Learned dismissal patterns with a confidence ladder (candidate, recurring, strong) and a skip / do-not-skip-when rule. | `cas-github-issues/SKILL.md` triage patterns | S |
| `skills/poteto-mode/scripts/check-plan.mjs` | Plan skeleton enforced by a script. | Skip: Cassy plans are task graphs with typed fields. | — |

**Hooks.** pstack ships none. It has no hooks.json, and `plugin.json` has only
`skills` and `agents`. It relies on `reminder:` frontmatter and one
always-applied model rule. This is where Cassy is clearly ahead: PreToolUse
guards, SessionStart context, Stop jobs and close gates. Nothing to adopt.

## Ranked adoptions

| Rank | Adopt | Cassy file | Effort |
|---|---|---|---|
| 1 | `cas-arena` (multi-model fan-out, cross-judge, pick, graft) | new `cas-cli/src/builtins/skills/cas-arena/SKILL.md` ×3 flavours plus `builtins.rs` registration | M |
| 2 | `cas-interrogate` (same prompt across model families, Act on/Consider/Noted/Dismissed, agreement map) | new `cas-cli/src/builtins/skills/cas-interrogate/SKILL.md` ×3; route from `cas-supervisor/references/workflow.md` | M |
| 3 | Blast-radius proof ladder plus one safety fact | `verify-before-claim/SKILL.md`, `agents/task-verifier.md` | S |
| 4 | Retry by failure mode plus synthetic failure handoff | `cas-supervisor/references/worker-recovery.md` | S |
| 5 | Worker brief fields, refuse-to-spawn; PASS/ISSUES/BLOCKED report contract with SHA and method | `cas-supervisor/references/workflow.md`, `cas-worker.md` | S (M with runtime check) |
| 6 | Patch-id re-verify rule after rebase | `cas-supervisor/references/workflow.md` | S |
| 7 | End-of-epic cross-model "Attention" review of decision notes; `evidence=` pointers in decision notes | `cas-supervisor/references/epic-driving.md`, `cas-worker.md` | S |
| 8 | Why workflow: epistemics tiers, coverage map with nulls, on top of `search history/blame` provenance | `cas-search.md` or new `cas-why/SKILL.md` | M |
| 9 | Reflect criteria (durability, structural-mechanism check → task, skill-was-used) | `cas-cli/src/builtins/jobs/learning-reviewer.md`, `session-learn/SKILL.md` | S |
| 10 | Project verification harness plus feature map | `cas-qa-craft/references/verify-harness.md`, `docs/qa/` template | M/L |
| 11 | figure-it-out loop: falsifiable predicate, baseline first, VERIFIED/NOT VERIFIED/INCONCLUSIVE | `cas-supervisor/references/planning.md` | S |
| 12 | Recall status tags for handoffs | `cas-memory-management/references/body-templates.md` | S |
| 13 | Architect scrap tells and design red flags | `cas-codebase-design/SKILL.md` | S |
| 14 | TDD skip conditions and bad-test definition | `cas-tdd/SKILL.md` | S |
| 15 | Repro-twice rule | `cas-diagnosing-bugs/SKILL.md` | S |
| 16 | Unslop rules 7 and 26 as a builtin text lint | `cas-cli/tests/builtin_doc_hygiene_test.rs` | M |
| 17 | Blinded skill-change eval | `cas-writing-for-agents/SKILL.md` (needs rank 1) | M |

Skipped: poteto-mode (as a whole), how, automate-me, teach, bro,
setup-pstack, no-comments, technical-writing, typescript-best-practices,
make-bot-ui, check-plan.mjs. Reasons are in each entry above.

## Coverage

Every non-principle skill under `skills/` is mapped above: poteto-mode,
architect, arena, interrogate, how, why, reflect, automate-me, figure-it-out,
show-me-your-work, swarm, recall, teach, tdd, bro, setup-pstack, blast-radius,
unslop, no-comments, technical-writing, typescript-best-practices,
make-bot-ui, create-verification-skill and maintain-verification-skill. The
23 `principle-*` skills are out of scope for this lane.

Also covered: `agents/`, the 23 playbooks, the poteto-mode scripts,
`automations/benny`, and the `orchestrate` plugin. The brief asked for
pstack's `hooks/`; that directory does not exist.

## Method

- I read every non-principle SKILL.md in full.
- I read the references for architect, interrogate, why and reflect selectively, and quoted them where cited.
- Two read-only research subagents summarised the 23 playbooks, the scripts, the agents and the benny automation, the orchestrate plugin, and the talk. Their citations were spot-checked against the files quoted above.
- Cassy facts are from the builtins at `379c8f277`.
- No code was built or changed.

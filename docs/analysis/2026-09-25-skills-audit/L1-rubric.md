# Skills & prompts audit rubric (EPIC cas-1660, lane L1 → used by L2–L5)

Version 1.1 (added cited limits + wrong-flavour = P1) · 2026-09-25 · author quick-kestrel-65 (task cas-63c5) · status: published early; citations
are being finalised in `findings.md` next to this file. A later edit will only add citations and
re-anchor, not change the scoring axes.

Score every skill, reference file, agent definition and injected prompt against the eight axes
below. For each finding give severity (P0–P3), `file:line`, evidence, a proposed fix, and an
estimated token delta (≈ bytes ÷ 4; negative = saving). Mark the shipped surface:
`always` (injected every session/turn: SessionStart guidance, spawn prompts, CLAUDE.md block,
skill-listing descriptions), `per-invoke` (SKILL.md body), `on-demand` (references/, scripts).

## Severity

| Sev | Meaning |
|---|---|
| P0 | Misleads an agent today: wrong tool/param/status/command, contradiction with the harness or the code, a file the harness cannot load, a script that never updates. |
| P1 | Routing or format defect: description that will not trigger (or double-fires), frontmatter invalid or ignored for a harness, a harness loading another harness's flavour (wrong tool prefix), always-loaded text over budget, harness-enforced rule restated in always-loaded text. |
| P2 | Efficiency / structure: duplication, stance before procedure, references that do not earn their pointer, stale narration. |
| P3 | Polish. |

Do not re-report items already fixed per `docs/analysis/2026-09-02-builtin-skills-review.md`;
do report regressions of those items as P0/P1.

## Axis 1 — Frontmatter validity (per harness)

Portable core (Agent Skills open standard, agentskills.io; Claude Code; Codex; Grok; OpenCode):

- `name`: lowercase letters, digits, hyphens; ≤ 64 chars; no leading/trailing/double hyphen;
  **must equal the directory name** (OpenCode and the open-standard validator reject a mismatch).
- `description`: non-empty, ≤ 1024 chars (hard spec limit). It is the only routing signal.
- Optional standard keys: `license`, `compatibility` (≤ 500 chars), `metadata` (string→string map),
  `allowed-tools` (experimental in the standard).
- Harness-specific keys are fine **only** in the harness that reads them:
  Claude Code/Grok read `disable-model-invocation`, `user-invocable`, `argument-hint`, `model`,
  `allowed-tools`; Grok additionally `when-to-use`, `effort`. `disallowed-tools` is a Claude Code
  **subagent** field (`disallowedTools`) — on a skill it is only meaningful if a harness documents it;
  score as P1 "unverified/ignored" unless the auditor cites the harness doc that honours it.
- Unknown top-level keys (e.g. CAS's `managed_by: cas`) are ignored by Claude Code/Grok but fail
  strict open-standard validation (`skills-ref validate`) and are the kind of key Codex/OpenCode may
  warn on. Preferred portable form: `metadata: { managed_by: cas }` (CAS's `is_managed_by_cas`
  substring check still matches). Score top-level custom keys P2, not P0.
- Reference files (`references/*.md`) must **not** carry skill frontmatter (`name:`/`description:`);
  Grok/Codex walk directories recursively and may register them as extra skills. P1.

## Axis 2 — Description / trigger quality (always-loaded)

- Lead with the trigger: `Use when <situation/user phrase>…` then the scope boundary. Third person,
  no "I/you can". Name concrete trigger phrases and file types; state the "not for X" boundary
  when a sibling or harness-bundled skill competes.
- Budget: every description is paid on every turn in every harness. Target ≤ 250 chars; > 400 is P1.
  Cited limits (findings.md §1): hard max 1,024 (spec/API/Codex/OpenCode); Claude Code truncates
  description+`when_to_use` at 1,536 and caps the whole listing at 1% of context (least-invoked
  skills lose descriptions first); Codex caps the listing at 2% of context or 8,000 chars and
  shortens descriptions first; OpenAI (2026-09-11): "as short as possible".
- Shape: `<what it does>. Use when <trigger>; not for <sibling>.` — key use case first.
- Opt-out portability: `disable-model-invocation` is honoured by Claude/Grok only; Codex needs
  `agents/openai.yaml policy.allow_implicit_invocation: false`; OpenCode ignores it.
  `disallowed-tools` is Claude-only and turn-scoped ("cleared when the user sends the next message") — not a guard.
- Wording (both vendors, 2026): no verification/"double-check"/"be thorough" scaffolding (Opus 5
  over-verifies); no vague ask-first/blocking language and no conflicting rules (GPT-6 stalls);
  no show-your-reasoning instructions (Fable `reasoning_extraction`).
- No shouting in descriptions ("ONLY", "MUST", "CRITICAL"); use `disable-model-invocation: true`
  for opt-in skills instead of shouted opt-in.
- No collision with a harness-bundled skill name or trigger set (Claude `dataviz`, Grok built-in
  slash commands such as `/release-notes` → forces `user:`/`local:` qualified names).

## Axis 3 — Progressive disclosure & size

- SKILL.md body: < 500 lines and ideally < ~5 k tokens (Anthropic best practice); Grok hard-caps
  inlining at 25 k tokens. CAS house budget: ~80 lines for methodology skills (cas-writing-for-agents).
- Always-loaded guidance (supervisor/worker SessionStart) must fit its byte budget
  (`SESSION_START_BUDGET_BYTES`, supervisor ≤ 8 KB) with room for degradable segments.
- References one level deep from SKILL.md; each linked with *when* to read it. A reference under
  ~15 lines belongs inline. Long references (> 100 lines) need a contents list at the top.
- Scripts are executed, not read: prefer "run `scripts/x`" over pasting code into the body.

## Axis 4 — Instruction wording for current models (Claude 5 family / Opus 5.x, GPT-5.x/6)

- Current frontier models follow instructions literally and over-apply emphasis. ALL-CAPS,
  "CRITICAL/IMPORTANT/NEVER/ALWAYS/MUST" on non-safety rules → P2 (P1 when in always-loaded text):
  rewrite as a plain imperative plus the reason.
- State what to do, not only what not to do; a prohibition earns space only for a hard guardrail,
  and should carry its reason (models generalise from the why).
- Examples must match the desired behaviour exactly (models copy details of examples); one good
  example beats three near-duplicates; label anti-examples explicitly.
- XML tags or clear headings to separate instructions, context and templates; consistent terms.
- No "think step by step"/"be thorough" boilerplate — reasoning effort is a harness setting; no
  instructions that fight tool-use defaults (e.g. begging to use a tool the harness already routes).
- Each rule stated once; don't restate what a hook/PreToolUse denial already enforces.

## Axis 5 — Procedure & completion (agentic effectiveness)

- Imperative numbered steps in execution order; the first actionable step within the first ~20
  lines of the body (stance/background after, or in references).
- Every step has an observable done-state; the skill ends with a `Done when …` criterion.
- Explicit stop/escalate conditions and a handoff (who gets what, where the evidence goes).
- Tools named by exact name and parameter shape for **this harness's** prefix.

## Axis 6 — Accuracy against the code

- Every `mcp__cas__<tool> action=<x>` and param exists in `cas-cli/src/mcp/tools/service/mod.rs`
  dispatch and `types/*.rs` schemas; every CLI command exists in `cas <cmd> --help`.
- No retired vocabulary: `pending_supervisor_review`, `bypass_code_review`, persona layer,
  `/epic-spec`, `/plan`, `cas-code-review`, "Phase 1/2", dated "verified on this machine" notes.
- No operator-specific facts (e-mails, `/home/<user>`, source-tree-only `../../../../` links).

## Axis 7 — Harness parity (Claude canonical → Codex → Grok → OpenCode projection)

Classify each divergence:

- **Intended adaptation**: tool-prefix substitution (`mcp__cas__` → `mcp__cs__` / `cas__` /
  `cas_`), harness-named checklist twins declared in `ALLOWED_FLAVOR_ONLY`, heterogeneous-team
  headings (`CANON_HETERO`), harness-specific frontmatter the target harness documents.
- **Drift**: any other textual difference, a file present in one catalog only without an
  allow-list entry, frontmatter a harness ignores or rejects, or an installed copy that differs
  from the shipped source after `cas update`.
- Check the *resolved* copy, not just the source: a harness that also scans another harness's
  directories (Grok scans `.claude/skills`, `.agents/skills`, `.cursor/skills`) may load the
  wrong flavour. Use `grok inspect --json` / install dirs as evidence.

## Axis 8 — Token economy

For every finding estimate the token delta and the multiplier: `always` × turns × sessions,
`per-invoke` × invocations, `on-demand` × reads. Rank P2 efficiency findings by
(delta × multiplier), not by raw size.

## Output shape per lane

`findings.md` table columns: `Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens`.
End with a search manifest (commands run + hit counts; 0-hit greps are evidence).

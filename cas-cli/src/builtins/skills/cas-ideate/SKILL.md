---
name: cas-ideate
description: Use when the user asks for new grounded project-improvement ideas ("what should I improve?", "surprise me", or "what would you change?") rather than refinement of an existing idea.
managed_by: cas
disallowed-tools:
  - Write
  - Edit
  - NotebookEdit
---

# Adversarial Ideation

Cassy pipeline position:

- `cas-ideate` answers: **"What are the strongest ideas worth exploring?"** (many → critique → survivors)
- `cas-brainstorm` answers: **"What exactly should one chosen idea mean?"** (Q&A → requirements)
- planning answers: **"How should it be built?"** (structured implementation plan)

This skill produces a ranked ideation artifact at `docs/ideation/YYYY-MM-DD-<topic>-ideation.md`. It does **not** produce requirements, plans, or code. After survivors are presented, acting on an idea hands off to `cas-brainstorm`, never directly to planning or execution.

**IMPORTANT: All file references in generated documents must use repo-relative paths** (e.g., `cas-cli/src/main.rs`), never absolute paths. Absolute paths break portability across worktrees, machines, and teammates.

## Core Principles

1. **Ground before ideating** — Scan the actual codebase first. Do not generate abstract product advice detached from the repository. Every idea must name a file, module, pattern, or observed friction as its hook.
2. **Generate many → critique all → explain survivors only** — The quality mechanism is **explicit rejection with reasons**, not optimistic ranking. Do not let extra process obscure this pattern.
3. **Route action into brainstorming** — Ideation identifies promising directions; `cas-brainstorm` defines the selected one precisely enough for planning. Do not skip to planning from ideation output.
4. **Rejection is the point** — A good ideation run cuts more ideas than it keeps. If everything survives, you weren't being adversarial enough.

## Interaction Method

Use `AskUserQuestion` when asking the user a question. Ask one question at a time. Prefer concise single-select choices when natural options exist. In factory mode AskUserQuestion is blocked — ask the questions in plain text and end your turn; the director relays answers.

## Focus Hint

If the user invoked the skill with an argument, interpret it as optional context. It may be:

- a concept such as `DX improvements`, `error handling`, `test ergonomics`
- a path such as `cas-cli/src/cli/`
- a constraint such as `low-complexity quick wins`, `no new dependencies`
- a volume hint such as `top 3`, `100 ideas`, `raise the bar`, `go deep`

If no argument is provided, proceed with open-ended ideation across the whole project.

---

## Execution Flow

### Phase 0: Resume and Scope

#### 0.1 Check for recent ideation work

Look at `docs/ideation/` for ideation documents created within the last 30 days, and search Cassy memory for prior ideation sessions:

```bash
ls docs/ideation/ 2>/dev/null
```
```
mcp__cas__search action=search query="ideation <focus>" doc_type=entry limit=5
```

A prior ideation doc is relevant when:
- The topic matches the requested focus
- The path or subsystem overlaps the requested focus
- The request is open-ended and there's an obvious recent open ideation doc

If a relevant doc exists, ask (via `AskUserQuestion`): "Found an existing ideation doc for [topic]. Continue from it, or start fresh?" In factory mode AskUserQuestion is blocked — ask the question in plain text and end your turn; the director relays answers.

If continuing:
- Read the document
- Summarize what has already been explored
- Preserve previous idea statuses and session log entries
- Update the existing file instead of creating a duplicate

#### 0.2 Interpret focus and volume

Infer two things from the argument:

- **Focus context** — concept, path, constraint, or open-ended
- **Volume override** — any hint that changes candidate or survivor counts

**Default volume:**
- Each ideation sub-agent generates ~8-10 ideas (yielding ~30 raw ideas across agents, ~20-25 after dedupe)
- Keep the top **5-7 survivors**

**Honor clear overrides** such as `top 3`, `100 ideas`, `go deep`, `raise the bar`. Use reasonable interpretation rather than formal parsing.

### Phase 1: Codebase Scan (Grounding)

Before generating ideas, gather codebase context. Ideas detached from the actual repo are the #1 failure mode of ideation.

Run two grounding steps in parallel (in the **foreground** — results are needed before Phase 2):

**1. Quick context scan** — dispatch a general-purpose sub-agent (cheap model is fine, e.g. Haiku) with this prompt:

> Read the project's `CLAUDE.md` and `README.md` (or `AGENTS.md` if present), then discover the top-level directory layout using Glob with pattern `*` or `*/*`. Return a concise summary (under 30 lines) covering:
> - Project shape (language, framework, top-level directory layout)
> - Notable patterns or conventions
> - Obvious pain points or gaps
> - Likely leverage points for improvement
>
> Keep the scan shallow — read only top-level documentation and directory structure. Do not do deep code search.
>
> Focus hint: {focus_hint}

**2. Cassy memory/learnings search** — run directly (not via sub-agent):

```
mcp__cas__search action=search query="<focus or general pain points>" doc_type=entry limit=15
mcp__cas__search action=search query="<focus>" doc_type=rule limit=10
```

Pull out any bugfix memories, architecture notes, or feedback entries that suggest known friction, fragile subsystems, or recurring problems.

**Consolidate into a short grounding summary:**

- **Codebase context** — project shape, notable patterns, observable pain points, likely leverage points
- **Past learnings** — relevant Cassy memories (bugfixes, architecture notes, feedback entries)
- **Known friction** — anything flagged as "this is annoying", "we keep hitting", "fragile", etc.

Do **not** do external research in v1 of this skill.

### Phase 2: Divergent Ideation

**Generate the full candidate list before critiquing any idea.** Mixing generation and critique destroys diversity — ideas get killed before their siblings can build on them.

Dispatch **3-4 parallel ideation sub-agents** on the inherited model (do NOT tier down — creative ideation needs the orchestrator's reasoning level). Each targets ~8-10 ideas (yielding ~30 raw ideas, ~20-25 after dedupe). Adjust per-agent targets when volume overrides apply.

Give each sub-agent:
- The grounding summary from Phase 1
- The focus hint (if any)
- Per-agent volume target
- Instruction to generate **raw candidates only, not critique**
- Instruction: *the first few ideas you think of are the obvious ones — push past them*
- Instruction to ground every idea in the Phase 1 scan (cite a file, module, or observed friction)

Assign each sub-agent a different **thinking frame** as a *starting bias, not a constraint*. Cross-cutting ideas that span multiple frames are valuable.

**Default frames (open-ended ideation):**

1. **User/operator pain and friction** — what makes daily work annoying, slow, or error-prone?
2. **Inversion, removal, or automation** — what painful step could be automated away, inverted, or removed entirely?
3. **Assumption-breaking / reframing** — what assumption is the project making that might not need to hold? What if the opposite were true?
4. **Leverage and compounding effects** — what small change would make many future changes easier? What unlocks downstream work?

**Compact structure per idea** (each sub-agent returns):

```yaml
- title: <short phrase>
  summary: <1-2 sentences>
  why_it_matters: <the pain or leverage>
  grounding: <file/module/memory this hooks into>
  boldness: <0-100, optional>
```

**After all sub-agents return:**

1. **Merge and dedupe** into one master candidate list. Collapse near-duplicates, preferring the better-grounded version.
2. **Synthesize cross-cutting combinations** — scan for ideas from different frames that combine into something stronger. Expect 3-5 additions at most, not 30.
3. **Weight toward the focus** if one was provided — but don't exclude stronger adjacent ideas.
4. **Spread across dimensions** — workflow/DX, reliability, extensibility, missing capabilities, docs/knowledge compounding, quality/maintenance, leverage on future work.

Then read `references/post-ideation-workflow.md` for the adversarial filtering rubric, presentation format, artifact template, and handoff. **Do not load that file before Phase 2 completes** — it would anchor critique thinking during generation.

---

## Anti-Patterns (do not do these)

- ❌ Critiquing during generation — kills sibling ideas before they appear
- ❌ Abstract product advice with no file/module/memory hook
- ❌ Keeping every idea because "they all have merit" — the whole point is cutting
- ❌ Skipping straight to planning from ideation output (always hand off via `cas-brainstorm`)
- ❌ Tiering down the ideation sub-agents to a weaker model — creative work needs reasoning level
- ❌ Loading `references/post-ideation-workflow.md` before Phase 2 completes
- ❌ Stopping after the first obvious ideas — sub-agent prompts must push past them
- ❌ Lowering the survivor bar when <5 survive — report honestly instead

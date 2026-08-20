---
name: cas-brainstorm
description: Use when the user asks to brainstorm or clarify an ambiguous feature, problem, scope, or direction before planning or coding.
managed_by: cas
disallowed-tools:
  - Write
  - Edit
  - NotebookEdit
---

# Brainstorm Before You Build

Brainstorming answers **WHAT** to build through collaborative dialogue. It precedes planning (which answers **HOW**) and execution (which builds it).

Cassy agents tend to default to executing immediately. This skill exists to invert that bias: **80% understanding the problem, 20% capturing the answer.** If you finish a brainstorm having mostly talked rather than mostly listened, you did it wrong.

The durable output is a **requirements document** stored at `docs/brainstorms/YYYY-MM-DD-<topic>-requirements.md`, strong enough that planning does not need to invent product behavior, scope boundaries, or success criteria.

**This skill does not implement code.** It explores, clarifies, and documents decisions for later planning or execution.

**IMPORTANT: All file references in generated documents must use repo-relative paths (e.g., `cas-cli/src/main.rs`), never absolute paths.** Absolute paths break portability across worktrees, machines, and teammates.

## Core Principles

1. **Assess scope first** — Match ceremony to the size and ambiguity of the work. A one-line bug fix should not get 10 questions.
2. **Be a thinking partner, not an interviewer** — Suggest alternatives, challenge assumptions, propose what-ifs. Don't just extract answers.
3. **Resolve product decisions here** — User-facing behavior, scope boundaries, and success criteria belong in this workflow. Detailed implementation belongs in planning.
4. **Keep implementation out of the requirements doc by default** — No libraries, schemas, endpoints, file layouts, or code-level design unless the brainstorm itself is inherently architectural.
5. **Right-size the artifact** — Lightweight brainstorms may skip the document entirely. Standard and Deep get a fuller document. Do not add ceremony that doesn't help planning.
6. **Apply YAGNI to carrying cost** — Prefer the simplest approach that delivers meaningful value. Avoid speculative complexity, but low-cost polish worth keeping is fine.

## Interaction Rules (NON-NEGOTIABLE)

These exist because Cassy agents have a documented tendency to dump multiple questions at once and lead with solutions. Do not skip them.

1. **Ask ONE question at a time.** Never batch unrelated questions into one message. If you find yourself writing "Also,..." or "And another thing:" — stop. Send the first question, wait for the answer, then ask the next.
2. **Use the `AskUserQuestion` tool for blocking questions.** It is the platform's blocking question tool. Use it instead of presenting numbered options in chat whenever possible. Numbered chat options are a fallback only. In factory mode AskUserQuestion is blocked — ask the questions in plain text and end your turn; the director relays answers.
3. **Prefer single-select multiple choice.** Single-select is faster for the user than open-ended prose questions. Use it when picking one direction, one priority, or one next step.
4. **Use multi-select rarely and intentionally.** Only for compatible sets like goals, constraints, or non-goals that can all coexist. If prioritization matters, follow up by asking which selected item is primary.
5. **Ask what the user is thinking BEFORE offering ideas.** This surfaces hidden context and prevents the user from anchoring on AI-generated framings. "What have you already considered?" is more valuable than "Here are 5 options."
6. **Broad before narrow.** Start with problem, users, and value. Only narrow to constraints, exclusions, and edge cases after the big picture is clear.

## Frontier-round mechanics

Adapted from mattpocock/skills `grilling`, MIT © 2026 Matt Pocock. Treat unresolved choices as a design tree. Ask every currently settled frontier question in one numbered round, give a recommended answer, and defer dependent questions. Research facts with tools; reserve the user’s turns for decisions. Recompute until no material branch remains implicit.

## Output Guidance

- **Keep outputs concise.** Short sections, brief bullets, only enough detail to support the next decision.
- **Use repo-relative paths.** When referencing files, use paths relative to the repo root (e.g., `cas-cli/src/cli.rs`), never absolute paths.
- **Today's date is in the system prompt.** Use it when dating requirements documents.

## Feature Description

If the user provided a feature description with the invocation, use it. Otherwise ask:

> "What would you like to explore? Please describe the feature, problem, or improvement you're thinking about."

Do not proceed until you have a feature description.

---

## Execution Flow

### Phase 0: Resume, Assess, and Route

#### 0.1 Resume Existing Work

Check for an existing requirements document on this topic:

```bash
ls docs/brainstorms/ 2>/dev/null
```

Also check Cassy for prior brainstorms or tasks:

```
cas__search action=search query="<topic keywords>" doc_type=entry limit=5
```

If a recent matching `*-requirements.md` file exists, or the user references prior work:
- Read the document
- Confirm with the user (one question, `AskUserQuestion`): "Found an existing requirements doc for [topic]. Continue from this, or start fresh?" In factory mode AskUserQuestion is blocked — ask the question in plain text and end your turn; the director relays answers.
- If resuming, summarize the current state, continue from existing decisions and outstanding questions, and update the existing document instead of duplicating it.

#### 0.2 Assess Whether Brainstorming Is Even Needed

**Clear-requirements indicators:**
- Specific acceptance criteria already provided
- Existing pattern referenced to follow
- Exact expected behavior described
- Constrained, well-defined scope

**If requirements are already clear:** Keep it brief. Confirm understanding and present concise next-step options. Skip Phases 1.1 and 1.2 entirely — go straight to Phase 1.3 or Phase 3. Do not invent ceremony.

**If the request is a quick-help question, factual lookup, or single-step task:** Don't brainstorm. Answer directly and exit.

#### 0.3 Assess Scope

Use the feature description plus a light repo scan to classify:

- **Lightweight** — small, well-bounded, low ambiguity (e.g., "add a flag to suppress this warning")
- **Standard** — normal feature or bounded refactor with some decisions to make (e.g., "add a notes field to tasks")
- **Deep** — cross-cutting, strategic, or highly ambiguous (e.g., "rethink how we handle worker coordination")

If scope is unclear, ask ONE targeted question to disambiguate, then proceed.

**Match ceremony to scope:**

| Scope | Phase 1.1 scan | Phase 1.2 pressure test | Questions | Phase 2 approaches | Requirements doc |
|---|---|---|---|---|---|
| Lightweight | Quick search | 2-3 questions | 1-3 | Often skip | Optional/compact |
| Standard | Two passes | Standard set | 4-8 | 2-3 approaches | Yes |
| Deep | Two passes + adjacent | Standard + durability | 8+ | 2-3 with challenger | Yes, full template |

If you make Lightweight tasks heavy, users will stop using this skill. **The lightweight path should be nearly invisible.**

### Phase 1: Understand the Idea

#### 1.1 Existing Context Scan

Match depth to scope:

**Lightweight** — Quick search for the topic. If something similar exists, note it and move on.

**Standard and Deep** — Two passes:

*Constraint Check* — Read project instruction files (`CLAUDE.md`, `AGENTS.md` if present) for workflow, product, or scope constraints that affect the brainstorm. Search Cassy for prior decisions:
```
cas__search action=search query="<topic>" doc_type=entry
cas__task action=list status=closed
```

*Topic Scan* — Search for relevant terms in the codebase. Read the most relevant existing artifact (prior brainstorm, plan, spec, skill, or feature doc). Skim adjacent examples covering similar behavior.

**Two rules govern technical depth during the scan:**

1. **Verify before claiming.** When the brainstorm touches checkable infrastructure (database tables, routes, config files, dependencies, modules), read the relevant source files to confirm what actually exists. Any claim that something is *absent* — a missing table, an endpoint that doesn't exist, a dependency not in `Cargo.toml`, a config option with no current support — must be verified against the codebase. If unverified, label it as an unverified assumption.

2. **Defer design decisions to planning.** Implementation details (schemas, migration strategy, endpoint structure, deployment topology) belong in planning, not here — *unless* the brainstorm is itself about a technical or architectural decision, in which case those details ARE the subject.

If nothing obvious appears after a short scan, say so and continue.

#### 1.2 Product Pressure Test

Before generating approaches, challenge the request to catch misframing. **This is the highest-leverage step in the entire skill.** Match depth to scope:

**Lightweight:**
- Is this solving the real user problem?
- Are we duplicating something that already covers this?
- Is there a clearly better framing with near-zero extra cost?

**Standard:**
- Is this the right problem, or a proxy for a more important one?
- What user or business outcome actually matters here?
- What happens if we do nothing?
- Is there a nearby framing that creates more value without more carrying cost?
- Given the current project state and goal, what is the single highest-leverage move right now: the request as framed, a reframing, an adjacent addition, a simplification, or doing nothing?
- Use the result to *sharpen* the conversation, not bulldoze the user's intent.

**Deep** — Standard questions plus:
- What durable capability should this create in 6-12 months?
- Does this move the product toward that, or is it only a local patch?

**You don't have to ask all of these out loud.** Most should be silent self-checks. Surface the ones that produce real tension with the request.

#### 1.3 Collaborative Dialogue

This is the heart of the skill. Follow the Interaction Rules above. **Use `AskUserQuestion` for questions whenever possible.** In factory mode AskUserQuestion is blocked — ask the questions in plain text and end your turn; the director relays answers.

**Guidelines:**
- Ask what the user is already thinking before offering your own ideas
- Start broad (problem, users, value) → narrow (constraints, exclusions, edge cases)
- Clarify the problem frame, validate assumptions, ask about success criteria
- Make requirements concrete enough that planning won't need to invent behavior
- Surface dependencies or prerequisites only when they materially affect scope
- Resolve product decisions here; leave technical implementation choices for planning
- Bring ideas, alternatives, and challenges — don't just interview

**Exit condition:** Continue until the idea is clear OR the user explicitly wants to proceed.

### Phase 2: Explore Approaches

If multiple plausible directions remain, propose **2-3 concrete approaches** based on research and conversation. If one approach is clearly best and alternatives are not meaningful, skip the menu and state the recommendation directly.

**Use at least one non-obvious angle.** The first approaches that come to mind are usually variations on the same axis. Force diversity:
- **Inversion** — what if we did the opposite?
- **Constraint removal** — what if X weren't a limitation?
- **Analogy** — how does another domain solve this?

**Present approaches FIRST, then evaluate.** Let the user see all options before hearing which one is recommended. Leading with a recommendation anchors the conversation prematurely.

When useful, include one deliberately **higher-upside challenger** option — an adjacent reframing that would most increase usefulness or compounding value without disproportionate carrying cost. Present it alongside the baseline, not as the default. Omit it when the baseline is clearly the right move.

For each approach:
- Brief description (2-3 sentences)
- Pros and cons
- Key risks or unknowns
- When it's best suited

After presenting all approaches, state your recommendation and explain why. Prefer simpler solutions when added complexity creates real carrying cost, but don't reject low-cost, high-value polish.

If relevant, call out whether the choice is:
- Reuse an existing pattern
- Extend an existing capability
- Build something net new

### Phase 3: Capture the Requirements

Write or update a requirements document only when the conversation produced durable decisions worth preserving.

Read `references/requirements-capture.md` for the document template, formatting rules, completeness checks, and the requirements grouping rule.

For **Lightweight** brainstorms, keep the document compact. Skip document creation entirely when the user only needs brief alignment and no durable decisions need to be preserved.

After writing the document, also store the topic and key decisions in Cassy memory so future brainstorms can find them:

```
cas__memory action=remember title="Brainstorm: <topic>" content="<1-paragraph summary + decisions + path to doc>" tags=brainstorm,<topic>
```

### Phase 4: Handoff

Read `references/handoff.md` for the option logic, blocked-handoff pattern, and closing summary format.

**The blocked-handoff pattern is critical:** if the requirements doc has any items under `Resolve Before Planning`, you must NOT offer "Proceed to planning" or "Proceed directly to work". Either ask the blocking questions now (one at a time), or pause the brainstorm with an explicit blocked status. This prevents accumulating unknowns that blow up during implementation.

---

## Anti-Patterns (do not do these)

- ❌ Dumping 5 questions in one message
- ❌ Leading with a recommendation before the user has seen alternatives
- ❌ Inventing requirements the user didn't state instead of asking
- ❌ Adding a heavy ceremony pass to a one-line bug fix
- ❌ Putting implementation details (libraries, schemas, endpoints) in the requirements doc
- ❌ Closing the brainstorm with `Resolve Before Planning` items still unanswered, then handing off to planning anyway
- ❌ Claiming something is missing from the codebase without grepping for it first
- ❌ Asking all the questions you can think of "just to be thorough" instead of the questions that actually unblock the next decision

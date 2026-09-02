---
managed_by: cas
---

# Requirements Capture

This content is loaded when Phase 3 begins — after the collaborative dialogue (Phases 0-2) has produced durable decisions worth preserving.

---

The requirements document behaves like a lightweight PRD without PRD ceremony. Include what planning needs to execute well, and skip sections that add no value for the scope.

The requirements document is for **product definition and scope control**. Do **not** include implementation details such as libraries, schemas, endpoints, file layouts, or code structure unless the brainstorm is inherently technical and those details are themselves the subject of the decision.

## Required content for non-trivial work

- Problem frame
- Concrete requirements or intended behavior with stable IDs
- Scope boundaries
- Success criteria

## Include when materially useful

- Key decisions and rationale
- Dependencies or assumptions
- Outstanding questions (split into Resolve Before Planning vs. Deferred to Planning)
- Alternatives considered
- High-level technical direction *only* when the work is inherently technical and the direction is part of the product/architecture decision

## Document Template

Use this template and omit clearly inapplicable optional sections:

```markdown
---
date: YYYY-MM-DD
topic: <kebab-case-topic>
---

# <Topic Title>

## Problem Frame
[Who is affected, what is changing, and why it matters]

## Requirements

**[Group Header]**
- R1. [Concrete requirement in this group]
- R2. [Concrete requirement in this group]

**[Group Header]**
- R3. [Concrete requirement in this group]

## Success Criteria
- [How we will know this solved the right problem]

## Scope Boundaries
- [Deliberate non-goal or exclusion]

## Key Decisions
- [Decision]: [Rationale]

## Dependencies / Assumptions
- [Only include if material]

## Outstanding Questions

### Resolve Before Planning
- [Affects R1][User decision] [Question that must be answered before planning can proceed]

### Deferred to Planning
- [Affects R2][Technical] [Question that should be answered during planning or codebase exploration]
- [Affects R2][Needs research] [Question that likely requires research during planning]

## Next Steps
[If `Resolve Before Planning` is empty: `→ Hand off to cas-supervisor for EPIC creation`]
[If `Resolve Before Planning` is not empty: `→ Resume cas-brainstorm to resolve blocking questions before planning`]
```

## Stable IDs

For **Standard** and **Deep** requirements docs, use stable IDs like `R1`, `R2`, `R3` so planning, tasks, and later review can refer to them unambiguously. Don't renumber when you add or remove requirements — IDs are stable for traceability.

For very small docs with only 1-3 simple requirements, plain bullets are acceptable.

## Grouping Requirements

When requirements span multiple distinct concerns, group them under bold topic headers within the Requirements section. **The trigger for grouping is distinct logical areas, not item count** — even four requirements benefit from headers if they cover three different topics.

- Group by logical theme (e.g., "Packaging", "Migration and Compatibility", "Contributor Workflow"), not by the order they were discussed
- Requirements keep their original stable IDs — numbering does not restart per group
- A requirement belongs to whichever group it fits best; do not duplicate it across groups
- Skip grouping only when all requirements are about the same thing

## Sizing the Doc

For **Standard** and **Deep** brainstorms, a requirements document is usually warranted.

For **Lightweight** brainstorms, keep the document compact, or skip document creation when the user only needs brief alignment and no durable decisions need to be preserved.

When the work is simple, combine sections rather than padding them. **A short requirements document is better than a bloated one.**

## Visual Communication

Include a visual aid when the requirements would be significantly easier to understand with one. Use sparingly.

| Requirements describe... | Visual aid | Placement |
|---|---|---|
| A multi-step user workflow or process | Mermaid flow diagram or ASCII flow | After Problem Frame, or under `## User Flow` heading |
| 3+ behavioral modes, variants, or states | Markdown comparison table | Within the Requirements section |
| 3+ interacting participants (roles, components, services) | Mermaid or ASCII relationship diagram | After Problem Frame, or under `## Architecture` |
| Multiple competing approaches being compared | Comparison table | In Phase 2 approach exploration |

**Skip visuals when:** prose communicates the concept clearly; the diagram would just restate requirements; the visual describes implementation architecture, schemas, state machines, or code structure (that belongs in planning); the brainstorm is simple and linear.

**Format selection:**
- **Mermaid** (default) for simple flows — 5-15 nodes, standard flowchart shapes. Use `TB` (top-to-bottom) so diagrams stay narrow.
- **ASCII/box-drawing** for annotated flows that need rich in-box content (CLI commands, decision branches, file path layouts). Follow 80-column max.
- **Markdown tables** for mode/variant comparisons.
- Place inline at the point of relevance, not in a separate section.
- Conceptual level only — user flows, information flows, mode comparisons.
- Prose is authoritative: when a visual and surrounding prose disagree, the prose governs.

## Pre-Finalization Checks

Before finalizing the document, ask yourself:

- What would planning still have to invent if this brainstorm ended now?
- Do any requirements depend on something claimed to be out of scope?
- Are any unresolved items actually product decisions rather than planning questions?
- Did implementation details leak in when they shouldn't have?
- Do any requirements claim infrastructure is absent without that claim having been verified against the codebase? If so, verify now or label as an unverified assumption.
- Is there a low-cost change that would make this materially more useful?
- Would a visual aid help a reader grasp the requirements faster than prose alone?

**If planning would need to invent product behavior, scope boundaries, or success criteria, the brainstorm is not complete yet.**

## File Placement

Ensure `docs/brainstorms/` directory exists before writing:

```bash
mkdir -p docs/brainstorms
```

File name: `docs/brainstorms/YYYY-MM-DD-<kebab-topic>-requirements.md`

## Outstanding Questions Discipline

If a document contains outstanding questions:

- Use `Resolve Before Planning` only for questions that **truly block** planning
- If `Resolve Before Planning` is non-empty, keep working those questions during the brainstorm by default
- If the user explicitly wants to proceed anyway, convert each remaining item into an explicit decision, assumption, or `Deferred to Planning` question before proceeding
- Do not force resolution of technical questions during brainstorming just to remove uncertainty
- Put technical questions, or questions that require validation or research, under `Deferred to Planning` when they are better answered there
- Use tags like `[Needs research]` when planning should investigate the question rather than answer it from repo context alone
- Carry deferred questions forward explicitly rather than treating them as a failure to finish the requirements doc

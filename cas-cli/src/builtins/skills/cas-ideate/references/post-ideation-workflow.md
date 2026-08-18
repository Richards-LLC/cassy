---
managed_by: cas
---

# Post-Ideation Workflow

Read this file **after** Phase 2 ideation agents return and the orchestrator has merged and deduped their outputs into a master candidate list. Do not load before Phase 2 completes — loading early anchors critique thinking during generation and kills diversity.

---

## Phase 3: Adversarial Filtering

Review every candidate idea critically. **The orchestrator performs this filtering directly — do not dispatch sub-agents for critique.** The whole point of this phase is that one adversarial reader applies a consistent bar, not that five polite sub-agents each keep their favorites.

**Do not generate replacement ideas in this phase** unless explicitly refining — that's Phase 2's job.

For every rejected idea, write a one-line reason. No silent cuts.

### Rejection criteria (apply ruthlessly)

- **Too vague** — can't tell what would actually change
- **Not actionable** — no clear owner, scope, or first step
- **Duplicates a stronger idea** — say which one
- **Not grounded in the current codebase** — abstract product wisdom with no repo hook
- **Too expensive relative to likely value** — high complexity, marginal win
- **Already covered** by existing workflows, docs, skills, or in-flight tasks
- **Interesting but better as a brainstorm variant** — belongs inside another idea, not as its own line item
- **Solves a problem nobody has** — speculative, no evidence of friction in Phase 1 grounding

### Scoring survivors

Score survivors on a consistent rubric weighing:

- **Groundedness** in the current repo (high = cites real files/modules/memories)
- **Expected value** — how much better does the project get?
- **Novelty** — would the user have thought of this themselves?
- **Pragmatism** — can this actually ship?
- **Leverage on future work** — does it unlock other improvements?
- **Implementation burden** — realistic estimate, not optimistic
- **Overlap with stronger ideas** — penalize partial duplicates

### Target output

- Keep **5-7 survivors** by default
- If too many survive, run a **second stricter pass** — raise the bar, don't just pick the top 7
- If **fewer than 5 survive**, report that honestly rather than lowering the bar. It's better to say "only 3 ideas cleared the bar" than to pad with weak ones

## Phase 4: Present the Survivors

Present the surviving ideas to the user **before writing the durable artifact**. This is a review checkpoint, not the final archived result.

Show only the survivors, in this structured form:

- **Title**
- **Description** — concrete explanation, not marketing
- **Rationale** — why this improves the project, grounded in Phase 1 scan
- **Downsides** — honest tradeoffs and costs
- **Confidence score** — 0-100%
- **Estimated complexity** — Low / Medium / High

Then include a **brief rejection summary** so the user can see what was considered and cut:

```
Rejected (12 ideas):
- "Rewrite CLI in Go" — too expensive, no grounding
- "Add plugin system" — duplicates stronger idea #3
- "Fancy TUI themes" — interesting, better as brainstorm variant
...
```

Keep the presentation concise. The durable artifact holds the full record.

Allow brief follow-up questions and lightweight clarification before writing the artifact.

**Do not write the ideation doc yet unless:**
- The user indicates the candidate set is good enough to preserve
- The user asks to refine and continue in a way that should be recorded
- The workflow is about to hand off to `cas-brainstorm` or session end

## Phase 5: Write the Ideation Artifact

Write the ideation artifact **after** the candidate set has been reviewed enough to preserve.

**Always write or update the artifact before:**
- Handing off to `cas-brainstorm`
- Ending the session

### Steps

1. Ensure `docs/ideation/` exists:
   ```bash
   mkdir -p docs/ideation
   ```
2. Choose the file path:
   - `docs/ideation/YYYY-MM-DD-<kebab-topic>-ideation.md` when a focus exists
   - `docs/ideation/YYYY-MM-DD-open-ideation.md` when ideation is open-ended
3. Write or update the ideation document using the template below.
4. Also store a Cassy memory so future sessions can find it:
   ```
   mcp__cas__memory action=remember title="Ideation: <topic>" content="Doc: <path>. Top survivors: <short list>. Run on <date>." tags=ideation,<topic>
   ```

### Artifact template

Use this structure. Omit clearly irrelevant fields only when necessary.

```markdown
---
date: YYYY-MM-DD
topic: <kebab-case-topic>
focus: <optional focus hint>
---

# Ideation: <Title>

## Grounding Summary

[Grounding summary from Phase 1 — codebase context, past learnings, known friction. Keep concise.]

## Ranked Ideas

### 1. <Idea Title>
**Description:** [Concrete explanation]
**Rationale:** [Why this improves the project, grounded in Phase 1]
**Downsides:** [Tradeoffs or costs]
**Confidence:** [0-100%]
**Complexity:** [Low / Medium / High]
**Grounding:** [`repo/relative/path.rs` or Cassy memory reference]
**Status:** [Unexplored / Explored]

### 2. <Idea Title>
...

## Rejection Summary

| # | Idea | Reason Rejected |
|---|------|-----------------|
| 1 | <Idea> | <Reason rejected> |
| 2 | <Idea> | <Reason rejected> |

## Session Log

- YYYY-MM-DD: Initial ideation — <candidate count> generated, <survivor count> survived. Focus: <focus hint or "open">
```

### If resuming

- Update the existing file in place
- Append to the session log
- Preserve `Explored` markers on ideas that were already picked up

## Phase 6: Refine or Hand Off

After presenting the results, ask (`AskUserQuestion`) what should happen next. In factory mode AskUserQuestion is blocked — ask the question in plain text and end your turn; the director relays answers.

**Options:**

1. **Brainstorm a selected idea** — hand off to `cas-brainstorm` with the idea as the seed
2. **Refine the ideation** — add more ideas, re-evaluate, or dig deeper on one
3. **Done for now** — keep the artifact, act later

### 6.1 Brainstorm a selected idea

If the user picks an idea:

- Write or update the ideation doc first (Phase 5)
- Mark that idea as `Explored` in the doc
- Append a session log entry: `YYYY-MM-DD: Selected idea #N for brainstorming`
- Invoke `cas-brainstorm` with the selected idea as the seed
- Also create a Cassy task pointing at both the ideation doc and the brainstorm-in-progress:
  ```
  mcp__cas__task action=create title="Brainstorm: <idea title>" description="Seed from docs/ideation/<file>.md idea #N" labels=brainstorm,from-ideation
  ```

**Do NOT skip brainstorming and go straight to planning from ideation output.** The ideation artifact is a list of directions, not a spec.

### 6.2 Refine the ideation

Route refinement by intent:

- `add more ideas` or `explore new angles` → return to Phase 2 with an instruction to avoid duplicating current candidates
- `re-evaluate` or `raise the bar` → return to Phase 3 with a stricter rubric
- `dig deeper on idea #N` → expand only that idea's description, rationale, and downsides

After each refinement:
- Update the ideation document before any handoff or session end
- Append a session log entry

### 6.3 Done for now

- Leave the artifact on disk (Phase 5 already wrote it)
- Do not create a git branch
- Do not commit or push
- The user can return later; the memory stored in Phase 5 will let future sessions find it

## Quality Bar

Before finishing, check:

- [ ] Every idea is grounded in the actual repo (cites a file, module, memory, or observed friction)
- [ ] The full candidate list was generated **before** filtering
- [ ] The many → critique → survivors mechanism was preserved (no cutting during generation)
- [ ] Sub-agents improved diversity without replacing the core workflow
- [ ] **Every rejected idea has a reason** — no silent cuts
- [ ] Survivors are materially better than a naive "give me ideas" list
- [ ] The artifact was written before any handoff or session end
- [ ] Acting on an idea routes to `cas-brainstorm`, not directly to planning or execution
- [ ] File paths in the artifact are repo-relative, never absolute

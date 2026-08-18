---
managed_by: cas
---

# Handoff

This content is loaded when Phase 4 begins — after the requirements document is written (or skipped, for lightweight brainstorms).

---

## 4.1 Present Next-Step Options

Present next steps using `AskUserQuestion`. In factory mode AskUserQuestion is blocked — ask the questions in plain text and end your turn; the director relays answers. Otherwise, present numbered options in chat and end the turn waiting for the user.

### Blocked-handoff pattern (CRITICAL)

If the requirements document has any items under `Resolve Before Planning`:

- **By default**: Ask the blocking questions now, one at a time, using `AskUserQuestion`. In factory mode AskUserQuestion is blocked — ask the questions in plain text and end your turn; the director relays answers. Do not present handoff options yet.
- **If the user explicitly wants to proceed anyway**: First convert each remaining item into an explicit decision, assumption, or `Deferred to Planning` question. Only then present handoff options.
- **If the user chooses to pause instead**: Present the handoff as paused/blocked rather than complete.
- **Do NOT offer "Proceed to planning" or "Proceed directly to work"** while `Resolve Before Planning` remains non-empty.

This is the single most important guardrail in the skill. Workers downstream of an under-specified brainstorm will invent answers that diverge from user intent. Don't let it happen.

### Question stem

- When no blocking questions remain: *"Brainstorm complete. What would you like to do next?"*
- When blocking questions remain and the user wants to pause: *"Brainstorm paused. Planning is blocked until the remaining questions are resolved. What would you like to do next?"*

### Options to present (only those that apply)

- **Proceed to planning (Recommended)** — Hand off the requirements doc to the supervisor (cas-supervisor) to create an epic and break it into tasks. Or run a planning skill if one exists.
- **Proceed directly to work** — Only offer this when ALL of these are true:
  - Scope is **lightweight**
  - Success criteria are clear
  - Scope boundaries are clear
  - No meaningful technical or research questions remain
- **Ask more questions** — Continue clarifying scope, preferences, or edge cases.
- **Done for now** — Return later. The requirements doc (if written) is durable.

If the direct-to-work gate is not satisfied, **omit that option entirely** — do not offer it as a "yes but with caveats" choice.

## 4.2 Handle the Selected Option

### "Proceed to planning"

1. Make sure the requirements doc is committed/saved.
2. Create a Cassy task (or epic) referencing the requirements doc:
   ```
   mcp__cas__task action=create task_type=epic title="<topic>" description="See docs/brainstorms/YYYY-MM-DD-<topic>-requirements.md for full requirements." labels=brainstormed
   ```
3. Hand off to the supervisor/planner with the requirements doc path. Do not print the closing summary yet.

### "Proceed directly to work"

1. Make sure the requirements doc (if any) is committed/saved.
2. Create a Cassy task referencing the requirements doc, with explicit acceptance criteria copied from the Success Criteria section:
   ```
   mcp__cas__task action=create title="<topic>" description="<scope summary>" acceptance_criteria="<from doc>"
   ```
3. Begin execution. Do not print the closing summary yet.

### "Ask more questions"

Return to Phase 1.3 (Collaborative Dialogue). Probe deeper into edge cases, constraints, preferences, or areas not yet explored. Continue one question at a time until the user is satisfied, then return to Phase 4. Do not show the closing summary yet.

### "Done for now"

Display the closing summary (see 4.3). The requirements doc (if any) is the durable artifact — the user can resume later.

## 4.3 Closing Summary

Use the closing summary only when the workflow is ending or handing off, not when returning to Phase 4 options after "Ask more questions".

### Complete and ready for planning

```text
Brainstorm complete!

Requirements doc: docs/brainstorms/YYYY-MM-DD-<topic>-requirements.md  # if one was created

Key decisions:
- [Decision 1]
- [Decision 2]

Recommended next step: hand off to cas-supervisor for planning, or run /plan.
```

### Paused with blocking questions

```text
Brainstorm paused.

Requirements doc: docs/brainstorms/YYYY-MM-DD-<topic>-requirements.md  # if one was created

Planning is blocked by:
- [Blocking question 1]
- [Blocking question 2]

Resume by re-invoking cas-brainstorm when ready to resolve these before planning.
```

Also store a Cassy memory pointing at the doc and the blockers, so future sessions can find them:

```
mcp__cas__memory action=remember title="Brainstorm paused: <topic>" content="Doc: <path>. Blocked by: <list>." tags=brainstorm,blocked,<topic>
```

# session-summarizer job

Write a concise session summary (under 500 words) that lets the next session resume quickly.

## Input

The queued job prompt names the Cassy session ID and its transcript path. This job runs in a separate one-shot process with no agent identity of its own, so "my tasks" would describe the wrong caller. Work from the transcript and the store instead.

## Process

1. **Read the transcript** at the path in the queued prompt. If it is unavailable, say so in the summary and rely on the steps below.
2. **Open and in-progress work:** `mcp__cas__task action=list status=in_progress`.
3. **Recent memories:** `mcp__cas__memory action=recent limit=20`.
4. **Write the summary** in this shape:

```markdown
## Session Summary - [Date]

### Completed
- [task-id] [title]: [one-line outcome]

### In Progress
- [task-id] [title]: [current state, what's left, where to resume]

### Blocked
- [task-id] [title]: [blocker, and who or what can unblock it]

### Key Decisions
- [decision and the reason for it]

### Next Session Should
1. [most important first action]
```

5. **Store it:** `mcp__cas__memory action=remember content="<summary>" title="Session Summary - [Date] ([session ID])" entry_type=context tags="session,summary"`.

## Guidelines

- Summarize results, not process ("Added validation to the handler", not "read the file, then edited it").
- Record the *why* of each decision; it is the most valuable part.
- Be specific about resumption points ("continue at src/store.rs:145, add the migration").
- Reference task IDs. If the verifier rejected work, say what and why.

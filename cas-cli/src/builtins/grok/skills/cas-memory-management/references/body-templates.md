---
managed_by: cas
---

# Cassy Memory Body Templates

Choose the template matching the memory's track. The track is determined by `problem_type` (see `references/schema.yaml`).

Both templates extend the legacy frontmatter (`name`, `description`, `type`) with the new structured fields. Existing memories without these fields continue working — the new fields are additive.

---

## Bug Track Template

Use for: `build_error`, `test_failure`, `runtime_error`, `performance_issue`, `database_issue`, `security_issue`, `integration_issue`, `logic_error`

```markdown
---
# Legacy fields (always required, kept for backward compatibility)
name: [Short human title]
description: [One-line summary used in MEMORY.md index]
type: bugfix

# Structured fields
track: bug
module: [crate or area, e.g. cas-mcp]
problem_type: [enum, e.g. runtime_error]
severity: [critical | high | medium | low]
root_cause: [enum, e.g. config_error]
symptoms:
  - [Observable symptom 1]
  - [Observable symptom 2]
resolution_type: [enum, e.g. config_change]    # optional
commit: [short SHA, e.g. 2dfe2aa]              # optional
tags: [keyword-one, keyword-two]               # optional, max 8
date: YYYY-MM-DD
---

## Problem
[1-2 sentences: what was broken and the user-visible impact]

## Symptoms
- [Observable symptom or error message — match the symptoms array above]
- [Include exact error strings, log lines, or reproduction steps]

## What Didn't Work
- [Attempted fix and why it failed — saves future debugging cycles]

## Solution
[The fix that worked. Include code snippets, config diffs, or commands.]

## Why This Works
[Root-cause explanation. Why the fix addresses the underlying issue, not just the symptom.]

## Prevention
- [Concrete practice, test, lint rule, or guardrail to prevent regression]

## Related
- [Related memories, commits, issues, or docs]
```

### Bug template guidance

- **Symptoms**: Observable, not interpretive. "MCP tool calls timeout after 60s" is good. "MCP is broken" is not.
- **What Didn't Work**: This is high-value content. Future agents will try the obvious fix first; document why it's wrong.
- **Why This Works**: Force yourself to articulate the root cause. If you can't, the fix may be a band-aid.
- **Prevention**: Prefer mechanical guardrails (test, lint, type) over "remember to do X".

---

## Knowledge Track Template

Use for: `best_practice`, `documentation_gap`, `workflow_issue`, `developer_experience`

```markdown
---
# Legacy fields
name: [Clear, descriptive title]
description: [One-line summary]
type: [project | reference | feedback | architecture]

# Structured fields
track: knowledge
module: [crate or area]
problem_type: [enum, e.g. best_practice]
severity: [critical | high | medium | low]
applies_when:                                 # optional
  - [Condition where this applies]
symptoms:                                     # optional — friction that prompted this
  - [Observable gap or friction]
tags: [keyword-one, keyword-two]              # optional, max 8
date: YYYY-MM-DD
---

## Context
[The situation, gap, or friction that prompted this guidance]

## Guidance
[The practice, pattern, or recommendation. Include code examples when useful.]

## Why This Matters
[Rationale and impact of following — or not following — this guidance]

## When to Apply
- [Conditions, file paths, or situations where this guidance kicks in]

## Examples
[Concrete before/after or usage examples]

## Related
- [Related memories, commits, or docs]
```

### Knowledge template guidance

- **Context**: Anchor the reader. Don't start with the rule; start with what made the rule necessary.
- **Guidance**: Concrete > abstract. "Use `git diff --quiet HEAD`" beats "use the right git command".
- **When to Apply**: Edge-case discipline. A rule with no scope becomes noise.

---

## Choosing a track

| You are documenting... | Track |
|---|---|
| A bug you fixed | bug |
| A test that broke and how you repaired it | bug |
| A perf regression and the cause | bug |
| A config or platform incompatibility | bug |
| A coding pattern the team uses | knowledge |
| A workflow or tooling preference | knowledge |
| Where to find external resources | knowledge (`type: reference`) |
| User preferences and feedback | knowledge (`type: feedback`) |

If the memory captures both ("we hit bug X, here's how to avoid it in general"), prefer **bug** track and put the general guidance in the Prevention section.

---

## Migration of legacy memories

Existing memories without structured fields remain valid. To upgrade a legacy memory:

1. Determine the track from its `type` field:
   - `type: bugfix` → `track: bug`
   - `type: project | reference | feedback | architecture | user` → `track: knowledge`
2. Infer `module` from the memory's content (which crate/area it discusses).
3. Pick a `problem_type` from the appropriate track's enum.
4. Set `severity` based on the original impact.
5. Add `date` from the memory's git history if not present in the body.
6. For bug-track upgrades, extract `symptoms` and `root_cause` from the existing body — these are usually already there in prose.

A future `cas memory migrate` command can backfill these fields semi-automatically.

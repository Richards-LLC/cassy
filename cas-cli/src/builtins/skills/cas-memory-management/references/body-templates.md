---
managed_by: cas
---

# Cassy Memory Content Templates

Pass the complete template as the `content` value of
`mcp__cas__memory action=remember`. The optional YAML block is frontmatter
inside the content stored in the SQLite entry; it is not a separate file.

## Bug or Incident

Use for a diagnosed build, test, runtime, performance, database, security,
integration, or logic problem.

```markdown
---
name: [short title]
description: [one-line summary]
module: [affected crate or area]
track: bug
root_cause: [diagnosed cause]
---

## Problem
[What was broken and who or what was affected]

## Symptoms
- [Observable error, behavior, or reproduction]

## What Didn't Work
- [Attempt that failed and why]

## Solution
[The change that fixed the problem]

## Why This Works
[The causal explanation, tied to the root_cause]

## Prevention
- [Test, guard, or practice that prevents recurrence]

## Related
- [Related entry IDs, commits, or docs]
```

## Knowledge or Guidance

Use for a durable practice, workflow improvement, reference, or developer
experience note. `track: knowledge` and `root_cause` may be omitted when they
do not add useful overlap metadata.

```markdown
---
name: [clear title]
description: [one-line summary]
module: [affected crate or area]
track: knowledge
---

## Context
[The situation or gap that prompted this guidance]

## Guidance
[The practice or pattern to apply]

## Why This Matters
[The impact of following the guidance]

## When to Apply
- [Relevant condition or scope]

## Examples
[Concrete usage examples]

## Related
- [Related entry IDs, commits, or docs]
```

## Writing Guidance

- Make `name` and `description` specific enough for overlap search.
- Put file paths, symbols, error text, and the actual explanation in the body.
- Use one `entry_type` request value (`learning`, `preference`, `context`, or
  `observation`) rather than inventing a second type vocabulary in content.
- Keep frontmatter valid YAML. The search and overlap readers are
  best-effort; malformed frontmatter leaves the body searchable but loses
  structured matching.

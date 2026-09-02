# Generated-doc hygiene

Shared by `codemap`, `project-overview` and `design-spec`. Each of those skills
regenerates one long-lived markdown artifact, and all three handle re-runs,
discoverability and freshness the same way. Read this once; the parent skill
names its own file, memory title and freshness command.

## 1. Preserve hand-edited sections

If the target file already exists:

1. **Read it first.**
2. **Preserve any `<!-- keep -->` … `<!-- /keep -->` blocks verbatim.** These are
   user-owned: do not rewrite, reflow, or even re-whitespace them. Put each one
   back in the section it appeared in.
3. Everything outside keep-blocks is regenerated.
4. If a section header has `<!-- keep -->` on the line directly below it,
   preserve that entire section including the header.

Both the bulleted lines and the `keep` markers survive re-runs. Destroying a
hand-edit is a trust breaker — the keep-block check is not optional.

## 2. Write a thin pointer memory

Invoke `cas__memory` with `action=remember` to create or update one pointer
memory, using the title the parent skill specifies.

- **Body:** ONE line. A repo-relative link to the doc plus a single-sentence hook.
- **No content duplication.** Do not inline the doc's contents. The point is that
  search surfaces the pointer and the reader opens the doc.

If a pointer with that title already exists, update it. Do not create duplicates.

## 3. Commit the doc

```bash
git add <the file you wrote>
git commit -m "docs: regenerate <the file you wrote>"
```

Commit it in the same change as anything else the run produced. Git history is
what later readers diff, and for `codemap` and `project-overview` it is also
what the freshness signal reads — so an uncommitted regeneration keeps nagging.

## 4. Report back

Print two things to the user: the file path that was written, and a three-bullet
summary. The parent skill says what the three bullets are.

---
name: cas-no-comments
description: Use when a delivery diff adds or changes comments, before a worker parks, or when asked to strip comments; reviews the diff only; not for adding comments or documenting code.
license: MIT
metadata:
  managed_by: cas
  author: Lauren Tan
  upstream: https://github.com/cursor/plugins/tree/main/pstack
  provenance: Adapted from pstack no-comments and comment-sicko (MIT, © 2026 Lauren Tan).
---

# No comments

A comment that explains surprising code goes stale while the code stays, and agents that read it later act on the stale claim. Delete the comment; if the code needed it, reshape the code.

## Steps

1. **Scope the diff.** Use the caller's files, else `git diff <target-branch>...HEAD` plus the working tree. Untouched files are out of scope. Done when the file list is written down.
2. **Run the review.** Spawn one subagent with [reviewer.md](references/reviewer.md) as its brief and the scope. Without subagents, follow the brief yourself. Done when you hold its report: files, deletion count, reshape flags, skips.
3. **Check the report.** Restore a deleted comment only when it matches a keep entry in the brief and you can name the symbol or path that proves it. Doubt means the deletion stands. Rerun once with the failure named; a second bad report goes in the close note as open.
4. **Act on reshape flags.** Make the smallest in-scope rename, extraction or type change that makes the code obvious without prose. When the fix is out of scope, name the flag in the close note. Never write a replacement comment.
5. **Offer an encoding for a claimed constraint.** For "do not remove" or "talk to X first", offer the cheapest in-scope test, type or lint that enforces it, then delete the comment; delete it and report the constraint open when the caller declines.

**Done when** the diff adds no comment outside the keep list and each reshape flag is fixed or named in the close note.

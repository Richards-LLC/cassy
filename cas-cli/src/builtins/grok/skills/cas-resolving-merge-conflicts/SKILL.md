---
name: cas-resolving-merge-conflicts
description: Use when resolving an in-progress git merge or rebase conflict.
managed_by: cas
---

# Resolve Merge Conflicts by Intent

Imported and adapted from mattpocock/skills `resolving-merge-conflicts`, MIT © 2026 Matt Pocock.

Inspect the conflict and history, trace each side to commits/PRs/Cassy tasks/specs, and preserve both intents where possible. If incompatible, follow the merge goal and record the trade-off in `mcp__cas__task`; never invent behavior or use `--abort` to evade the decision. Run affected checks and commit the completed resolution.

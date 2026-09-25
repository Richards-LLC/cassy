---
name: cas-resolving-merge-conflicts
description: Use when resolving an in-progress git merge or rebase conflict.
license: MIT
metadata:
  managed_by: cas
  author: Matt Pocock
  upstream: https://github.com/mattpocock/skills
  provenance: Adapted from mattpocock/skills resolving-merge-conflicts (MIT, © 2026 Matt Pocock).
---

# Resolve Merge Conflicts by Intent

1. Inspect the current merge or rebase state, history, and conflicting files.
2. Trace both sides to their primary sources: commits, pull requests, Cassy tasks, specs, and documented intent.
3. Resolve each hunk by preserving both intents where possible. If they are incompatible, choose the change matching the merge’s stated goal and record the trade-off with `task action=notes note_type=decision`. Do not invent new behavior.
4. Finish the merge or rebase; do not abandon it with `--abort` merely to avoid the decision.
5. Run the project’s affected checks, fix integration damage, then commit the resolved result. A factory worker on a lane that forbids builds (cargo is denied on Rust lanes) commits without running them and names the unverified surfaces in the task note; the supervisor’s `ASSEMBLY_PROOF` run checks them.

Use Cassy task/spec context for intent; do not introduce external tracker, scratch, setup, or context-file workflows.

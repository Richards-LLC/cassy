---
name: cas-writing-for-agents
description: Use when creating or editing a skill, AGENTS.md, CLAUDE.md, or an agent-facing reference document.
managed_by: cas
license: MIT
metadata:
  author: Matt Pocock
  upstream: https://github.com/mattpocock/skills
  provenance: Adapted from mattpocock/skills (MIT, © 2026 Matt Pocock).
---

# Writing for agents

Write agent-facing documents as instructions for a repeated process, not prose for a one-time reader. Read [skill mechanics](SKILL-MECHANICS.md) when the document is a skill.

## Context pointers

A **context pointer** names out-of-context material and the condition for loading it. A skill description and an `AGENTS.md` pointer are both pointers. State what the material is and each distinct branch that should load it. Front-load trigger words, use one trigger per branch, and omit identity the target already carries: every always-loaded word spends context load.

## The two loads

- **Context load** is always-loaded text. Keep it small and earn every word.
- **Cognitive load** is the human effort of knowing which document to reach for. Spend it where human judgment matters.

Material behind a pointer reduces context load but increases cognitive load.

## Information hierarchy

Put material at the lowest tier that still makes execution reliable:

1. In-file steps: actions every branch performs, in order.
2. In-file reference: rules consulted while doing those steps.
3. Disclosed reference: branch-specific material in a linked file.

Use progressive disclosure to keep the main path legible. Co-locate a concept's definition, rules, and caveats. Split only when a sequence or invocation branch earns the extra pointer; otherwise splitting becomes sprawl.

## Steps and completion criteria

Every step needs a completion criterion. Make it **clear** (done is observable) and **demanding** (the necessary legwork is required). Sharpen a fuzzy criterion before hiding later steps; only a real context boundary prevents later work from pulling attention toward premature completion.

## Leading words and pruning

Prefer compact, familiar leading words that reliably summon a shared behavior: `tight` for a fast deterministic loop, `red` for a bug-reproducing loop. Use positive instructions; a prohibition earns space only for a hard guardrail.

Keep one authoritative statement for each meaning. Treat environment facts as lookups, not prose caches, unless the lookup is costly or misses an unwritten convention. Remove stale exposition, irrelevant branches, and no-op instructions. The result should be short because every line is live.

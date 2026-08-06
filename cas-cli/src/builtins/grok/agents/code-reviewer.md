---
name: code-reviewer
description: DEPRECATED — replaced by the cas-code-review skill. Do not invoke. See cas-code-review for multi-persona review.
model: sonnet
managed_by: cas
deprecated: true
replaced_by: cas-code-review
---

# DEPRECATED

This agent has been **replaced by the `cas-code-review` skill** as of Phase 1
subsystem A (EPIC cas-0750). The single-pass pattern-grep reviewer it used to
run is superseded by a seven-persona pipeline with shared findings schema,
confidence gating, fingerprint dedup, cross-reviewer agreement boost, and
automatic task creation for residual findings.

If you were about to spawn this agent, invoke the **`cas-code-review` skill**
instead. The skill lives at `.grok/skills/cas-code-review/SKILL.md` after
`cas sync` and is dispatched automatically at factory worker `task.close`.

This file remains in `GROK_BUILTIN_AGENTS` solely so `cas sync` can overwrite
any stale downstream copies of the old agent with this stub. It will be
removed in a future release once downstream caches have expired.

- Replacement: `cas-code-review` skill
- EPIC: cas-0750
- Requirements: `docs/brainstorms/2026-04-09-multi-persona-code-review-requirements.md`

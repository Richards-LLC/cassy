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

Write agent-facing documents as instructions for a repeated process, not prose for a one-time reader.

## Steps

1. **Write the description as a context pointer.** Lead with `Use when <trigger>`, one trigger per branch that should load the document. Name the material and the condition; omit identity the target already carries. A description is always-loaded text — every word spends context on every turn.
2. **Write the body as imperative steps**, each with an observable done-state. A reader must be able to tell, without judgement, whether a step finished.
3. **State each rule exactly once.** One authoritative sentence per meaning. Delete any instruction the harness already enforces — a `PreToolUse` denial lands better than a paragraph asking nicely, and the paragraph costs context forever.
4. **Add a reference file only for a branch that earns it.** A branch earns one when it needs independent model invocation, or when a real invocation boundary protects a sequence from premature completion. Anything under ~15 lines belongs inline: a pointer to a short file costs more than the file saves.
5. **Update the codex and grok mirrors.** `cas-cli/src/builtins/skills/<name>/` is canonical; `codex/skills/<name>/` and `grok/skills/<name>/` are byte-identical to it apart from each harness's own CAS tool prefix, which is a pure substitution — never write another harness's prefix into a mirror. Register every new file in `BUILTIN_SKILLS`, `CODEX_BUILTIN_SKILLS` and `GROK_BUILTIN_SKILLS` in `builtins.rs` — an unregistered file is never installed — then regenerate `builtins/reference-history.json` with `scripts/gen-builtin-reference-history.sh`.

**Done when** the file is under ~80 lines, every line is live, `scripts/run-scoped-tests.sh -p cas --test builtin_flavor_drift_test` passes, and no sentence restates one that appears elsewhere in the same file.

## Frontmatter

Required: `name`, matching the directory; `description`, the `Use when …` pointer; `managed_by`, always the value `cas` for a builtin.

Optional: `disable-model-invocation` makes the skill user-invoked only. Write it as `disable-model-invocation: true`; its description then becomes human-facing and carries no autonomous trigger; choose it when a person should decide whether to use the skill, trading context load for cognitive load. `disallowed-tools` removes tools from the skill's session; never list a tool the skill's own steps require.

A family of user-invoked skills may share one user-invoked router: a router helps people find the right skill, but it cannot autonomously invoke its peers.

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

## Completion criteria

Every step needs one. Make it **clear** (done is observable) and **demanding** (the necessary legwork is required). Sharpen a fuzzy criterion before hiding later steps; only a real context boundary prevents later work from pulling attention toward premature completion.

## Leading words and pruning

Prefer compact, familiar leading words that reliably summon a shared behavior: `tight` for a fast deterministic loop, `red` for a bug-reproducing loop. Use positive instructions; a prohibition earns space only for a hard guardrail.

Treat environment facts as lookups, not prose caches, unless the lookup is costly or misses an unwritten convention. Remove stale exposition, irrelevant branches, and no-op instructions. Ticket-phase narration ("Phase 2", "v1 ships as advisory"), dated verification notes, and operator-specific facts — an e-mail address, an absolute home path, a downstream framework's conventions — are the reliable markers of text that has gone stale; they belong in project rubrics, config keys, or memories, never in a file installed for every user. The result should be short because every line is live.

---
name: cas-writing-for-agents
description: Use when creating or editing a skill, AGENTS.md, CLAUDE.md, or an agent-facing reference document.
license: MIT
metadata:
  managed_by: cas
  author: Matt Pocock
  upstream: https://github.com/mattpocock/skills
  provenance: Adapted from mattpocock/skills (MIT, © 2026 Matt Pocock).
---

# Writing for agents

Write agent-facing documents as instructions for a repeated process, not prose for a one-time reader.

## Steps

1. **Write the description as a context pointer:** `Use when <trigger>; not for <sibling>.`, where the trigger names what the skill does. Put the key use case first, keep it within 250 characters, and use no emphasis words. The 1,024-character limit is the hard ceiling, but every harness fits all descriptions into one shared listing budget (Claude: 1% of context; Codex: 2%, or 8,000 characters) and drops or shortens the longest first. Add the `not for` clause only when a sibling or bundled skill competes for the same prompts.
2. **Write the body as imperative steps**, each with an observable done-state. A reader must be able to tell, without judgement, whether a step finished.
3. **State each rule exactly once.** Delete any instruction the harness already enforces: a `PreToolUse` denial lands better than a paragraph asking nicely, and the paragraph costs context forever.
4. **Add a file only for a branch that earns it.** A reference earns one when it needs independent model invocation or a real invocation boundary; anything under ~15 lines stays inline. Keep references one level deep and give one over 100 lines a table of contents. Put executable helpers in `scripts/` and tell the agent to run them, not read them.
5. **Name Cassy tools by bare name** (`task action=start`, the `coordination` tool). Harnesses read each other's skill directories, so a prefixed name (`mcp__cas__task`) fails in the others; the role skills state each harness's prefix once. Spell a prefix only on a line that names its harness. `cas-cli/src/builtins/skills/<name>/` is the one copy: register every new file in `BUILTIN_SKILLS`, `CODEX_BUILTIN_SKILLS` and `GROK_BUILTIN_SKILLS` in `builtins.rs`, each embedding that file (an unregistered file is never installed), then run `scripts/gen-builtin-reference-history.sh`.

**Done when** the body fits its line budget (methodology skills under 80 lines, procedural skills under 200, never over 500 lines or ~5,000 tokens: split into references instead), every line is live, `builtin_flavor_drift_test` passes (supervisors run it; factory workers never build Rust), and no sentence restates another.

## Frontmatter by harness

| Field | Honoured by | Notes |
|---|---|---|
| `name`, `description`, `license`, `compatibility`, `metadata` | every harness and the open standard | `name` matches the directory. Only these (plus `allowed-tools`) pass claude.ai, the Skills API and the spec validator. |
| `metadata.managed_by: cas` | Cassy | Required for a builtin: sync overwrites only managed files. The legacy top-level `managed_by` still counts for one release. |
| `disable-model-invocation: true` | Claude, Grok | Makes the skill user-invoked only; its description becomes human-facing. Codex ignores `disable-model-invocation`: ship a Codex-only `agents/openai.yaml` with `policy: {allow_implicit_invocation: false}`. |
| `allowed-tools`, `argument-hint`, `when_to_use`, `paths`, `model`, `effort` | Claude, Grok | `allowed-tools` pre-approves; it never restricts. |
| `disallowed-tools`, `context`, `agent`, `hooks`, `shell`, `arguments` | Claude only | `disallowed-tools` is turn-scoped (cleared at the next user message), so it is not a guard; enforce a ban in a hook. |
| `user-invocable: false` | differs | Hidden from the user only in Claude, from the model too in Grok. Avoid it. |

A family of user-invoked skills may share one user-invoked router: a router helps people find the right skill, but it cannot invoke its peers.

## Wording for current models

- Use no capitals or emphasis words except for a true invariant, and at most one per file: current models over-trigger on them.
- Give the reason with each rule; a rule without one is applied too widely or too narrowly.
- Leave out verification scaffolding ("double-check", "be thorough", "run the tests again"); it causes over-verification.
- Tie every ask-first or stop instruction to a concrete trigger; open-ended caution stalls work.
- Make sure no two rules can conflict; models block or burn reasoning on a conflict.
- Examples are copied literally: give one exact example and label any anti-example as such.
- Never ask the model to show or echo its reasoning.

Say what to do rather than what not to do; a prohibition earns space only for a hard guardrail. Prefer compact, familiar leading words that summon a shared behaviour: `tight` for a fast deterministic loop, `red` for a bug-reproducing loop.

## Instruction files

`AGENTS.md` and `CLAUDE.md` are always loaded, so every line costs context in every session. Keep each under 200 lines. Codex reads `AGENTS.md` from the repo root down to the working directory, up to 32 KiB; Grok reads `AGENTS.md` and `CLAUDE.md` in full; OpenCode reads `AGENTS.md` instead of `CLAUDE.md`. Write blocks that ship in both files harness-neutral, and never duplicate a block across them.

## Loads and pruning

Context load is always-loaded text; cognitive load is the human effort of knowing which document to reach for. Material behind a pointer reduces the first and raises the second, so split only when a sequence or invocation branch earns the pointer, and co-locate a concept's definition, rules and caveats.

Treat environment facts as lookups, not prose caches. Remove stale exposition, irrelevant branches and no-op instructions. Ticket-phase narration ("Phase 2", "v1 ships as advisory"), dated verification notes and operator-specific facts (an e-mail address, an absolute home path, one downstream framework's conventions) mark text that has gone stale; they belong in project rubrics, config keys or memories, never in a file installed for every user.

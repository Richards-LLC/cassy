---
name: cas-domain-modeling
description: Use when clarifying a project's terminology, domain relationships, or durable technical decisions.
managed_by: cas
license: MIT
metadata:
  author: Matt Pocock
  upstream: https://github.com/mattpocock/skills
  provenance: Adapted from mattpocock/skills (MIT, © 2026 Matt Pocock).
---

# Domain modeling

Actively sharpen a project's shared domain model while designing. This Cassy
adaptation stores resolved language and decisions in the canonical CAS memory
and spec surfaces, never in a parallel `CONTEXT.md` or ADR-file convention.

## During the session

### Challenge the language

When a term conflicts with established project language, surface the conflict
immediately. When it is vague or overloaded, propose a precise canonical term.
Stress-test relationships with concrete edge cases, and verify claims about
behavior against the code before treating them as domain truth.

### Persist resolved terms

When a term is settled, store its concise definition with
`mcp__cas__memory action=remember`, using project scope and tags that make it
retrievable. Record the term, its boundaries, important synonyms to avoid, and
the scenario that disambiguated it. Search existing project memories first so
the entry refines rather than duplicates the canonical language.

### Persist consequential decisions

Use `mcp__cas__spec` or a decision memory only when the choice is hard to
reverse, surprising without context, and the result of a real trade-off. A
decision records the alternatives, chosen direction, reason, and affected
domain terms; it is not an implementation scratchpad. Link it to the active
task when it governs delivery.

## Completion

Before ending, ensure every resolved term or irreversible decision is either
durably recorded in Cassy or explicitly left open. Do not create glossary,
context-map, or ADR files that would compete with CAS memory/spec storage.

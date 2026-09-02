---
name: cas-codebase-design
description: Use when designing or restructuring a module, choosing a seam, assessing testability and architectural depth, or settling the project's domain terminology and durable technical decisions.
managed_by: cas
license: MIT
metadata:
  author: Matt Pocock
  upstream: https://github.com/mattpocock/skills
  provenance: Adapted from mattpocock/skills codebase-design (MIT, © 2026 Matt Pocock).
---

# Codebase design

Design deep modules: substantial behavior behind a small, clear interface at a
well-chosen seam. This vocabulary improves leverage for callers, locality for
maintainers, and tests that exercise real behavior.

## Vocabulary

- **Module**: anything with an interface and implementation, from a function
  to a package or vertical slice.
- **Interface**: every fact a caller must know: types, invariants, ordering,
  error modes, configuration, and performance characteristics.
- **Depth**: behavior a caller can exercise per unit of interface it must
  learn. A shallow module exposes nearly as much complexity as it hides.
- **Seam**: where behavior can vary without editing the caller; its placement
  is a design choice distinct from the implementation behind it.
- **Adapter**: a concrete implementation filling a seam. Use it when the
  varying slot matters; otherwise say implementation.
- **Leverage** and **locality**: the caller and maintainer benefits of depth.

Use this language consistently, except that established framework vocabulary
always wins: where the project's framework already gives a word a precise,
conventional meaning, that meaning is the project's language and this skill
does not override it.

## Challenge the language

Sharpen the shared domain model while you design. When a term conflicts with
established project language, surface the conflict immediately. When it is
vague or overloaded, propose a precise canonical term. Stress-test
relationships with concrete edge cases, and verify claims about behavior
against the code before treating them as domain truth.

When a term is settled, store its concise definition with
`mcp__cs__memory action=remember`, using project scope and tags that make it
retrievable: the term, its boundaries, synonyms to avoid, and the scenario that
disambiguated it. Search existing project memories first so the entry refines
rather than duplicates the canonical language. Never open a parallel glossary
or decision file that competes with CAS memory/spec storage.

## Principles

- Depth is a property of the interface, not implementation size.
- Apply the deletion test: if deleting a module merely removes a pass-through,
  it was shallow; if complexity reappears at many callers, it earned its keep.
- The interface is the primary test surface. Tests should assert observable
  outcomes through it, not reach through internal seams.
- One adapter is a hypothetical seam; two justified adapters make it real.
  Do not add indirection when nothing varies.
- Accept dependencies rather than creating them; return results where possible
  rather than hiding critical work in side effects.

## Design it twice

Before committing to a consequential interface, frame constraints, dependency
categories, and an illustrative sketch.
Explore at least three materially different interfaces:
minimum surface, maximum flexibility, and the common caller.
Compare depth, locality, seam placement, hidden complexity, and trade-offs.

## Deepening an existing shallow module

Classify dependencies before combining shallow modules: in-process dependencies
can be tested through the new interface directly; local-substitutable ones use
their real local stand-in; owned remote dependencies justify an injected port
and in-memory/transport adapters; true externals use an injected mock adapter.

Keep internal seams private. Replace old shallow-module tests with tests at the
deepened interface once that interface covers the behavior; do not layer tests
that assert internals or must change with every refactor.

## Completion

Done when a task note (`mcp__cs__task action=notes note_type=decision`) lists:
the chosen seam, the interface facts callers must learn, what complexity stays
hidden behind it, the deletion-test result, and two rejected alternatives with
the reason each lost. If the choice is hard to reverse, surprising without
context, and the result of a real trade-off, also record it with
`mcp__cs__spec action=create` — never in a parallel architecture-record
directory.

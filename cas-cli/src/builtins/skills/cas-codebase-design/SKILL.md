---
name: cas-codebase-design
description: Use when designing or restructuring a module, choosing a seam, or assessing testability and architectural depth.
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
always wins. In NestJS projects, `*.service.ts` and `service` are normal,
meaningful conventions; do not apply an "avoid service" rule there.

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

## Completion

Name the selected seam, the interface facts callers must learn, what complexity
stays behind it, and the deletion-test result. If the choice is consequential,
capture it in the active Cassy task or a `mcp__cas__spec` decision rather than a
parallel architecture-record directory.

For dependency categories and replacement testing, read
[DEEPENING.md](DEEPENING.md). For alternatives, read
[DESIGN-IT-TWICE.md](DESIGN-IT-TWICE.md).

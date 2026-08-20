---
name: cas-tdd
description: Test-driven development discipline for behavior-focused, scoped Cassy changes. Use when a task requires test-first work, red-green-refactor, or integration-test design.
managed_by: cas
---

# Test-Driven Development

Imported and adapted from mattpocock/skills `tdd`, MIT © 2026 Matt Pocock.

Use a red → green loop: agree public seams first, write one behavior-focused vertical slice, add the smallest implementation, then refactor after green. Keep decisions and proof in Cassy’s `mcp__cas__task`, `mcp__cas__spec`, and `mcp__cas__memory` surfaces.

Avoid implementation-coupled, tautological, and horizontal-slice tests. Mock external boundaries rather than internal collaborators merely to assert calls, and record a scoped proof with a nonzero test count.

**NestJS carve-out:** `Test.createTestingModule` provider overrides are sanctioned seams for injected external or separately-owned dependencies when public module behavior is under test.

---
name: cas-tdd
description: Test-driven development discipline for behavior-focused, scoped Cassy changes. Use when a task requires test-first work, red-green-refactor, or integration-test design.
managed_by: cas
---

# Test-Driven Development

Imported and adapted from mattpocock/skills `tdd`, MIT © 2026 Matt Pocock.

Use a red → green loop to produce tests worth keeping. Test public behavior, agree seams before testing, work one vertical tracer-bullet slice at a time, and use independently derived expected values. Keep decisions and proof in `mcp__cas__task`, `mcp__cas__spec`, and `mcp__cas__memory`, never parallel tracker or context files.

Avoid implementation-coupled, tautological, and horizontal-slice tests. Mock real system boundaries, not internal collaborators merely to assert calls. Run the project’s affected scoped test target and record its nonzero test count and exit status.

**NestJS carve-out:** `Test.createTestingModule` provider overrides are the framework-sanctioned seam for injected external or separately-owned dependencies. They are valid when the test exercises public module behavior; do not classify them as an implementation-coupled anti-pattern merely because Nest uses dependency injection.

For seam vocabulary, consult `cas-codebase-design`.

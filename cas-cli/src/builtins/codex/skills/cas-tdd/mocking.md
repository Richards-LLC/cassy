# When to Mock

Imported and adapted from mattpocock/skills `tdd/mocking.md`, MIT © 2026 Matt Pocock.

Mock external boundaries, not internal collaborators merely to prove calls. NestJS `Test.createTestingModule` provider overrides remain sanctioned seams when a public module behavior is under test.

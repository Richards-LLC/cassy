# When to Mock

Imported and adapted from mattpocock/skills `tdd/mocking.md`, MIT © 2026 Matt Pocock.

Mock at boundaries: third-party APIs, time, randomness, filesystems, and databases when an appropriate real test fixture is unavailable. Keep mocks specific to the boundary contract so a failing test identifies the behavior that changed.

Do not mock internal collaborators solely to assert that they were called. Test through the owning module’s public interface whenever that yields the behavior a caller cares about.

## NestJS provider seam

`Test.createTestingModule` and provider overrides are sanctioned NestJS seams. Mock an injected provider when it is an external system or separately-owned dependency and the test is asserting the module’s public behavior. Do not reject this pattern as an internal-collaborator mock merely because Nest resolves it through dependency injection.

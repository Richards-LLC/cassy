# When to Mock

Imported and adapted from mattpocock/skills `tdd/mocking.md`, MIT © 2026 Matt Pocock.

Mock at boundaries: third-party APIs, time, randomness, filesystems, and databases when an appropriate real test fixture is unavailable. Keep mocks specific to the boundary contract so a failing test identifies the behavior that changed.

Do not mock internal collaborators solely to assert that they were called. Test through the owning module’s public interface whenever that yields the behavior a caller cares about.

## Framework-provided seams

A framework's sanctioned test seam — a DI container's provider override, a test module builder — is a legitimate boundary. Mock an injected dependency through it when that dependency is an external system or separately-owned code and the test asserts the module's public behavior. Do not reject the pattern as an internal-collaborator mock merely because the framework resolves it through dependency injection.

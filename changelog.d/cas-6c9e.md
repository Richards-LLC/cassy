### Fixed — Web base attribution on macOS

- **Base attribution maps Playwright failures under symlinked temp roots** — On macOS, `/var` points to `/private/var`, and the base replay crashed with a doubled `/private/private` path. Failing tests now resolve to the base or the delivery. Before this fix, unrelated failures could not be attributed and parks needed manual triage.

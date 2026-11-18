## Authoritative Sources

Factory-lane own-test builds run on the trusted local `cas-lane-light` runners. Their existing evidence upload includes `target/ci-impact/own-build.log`, which captures compiler output, test execution and watchdog resource heartbeats without masking command failures. Ring shards and browser jobs keep their existing runners and stage budgets.

Self-hosted Rust setup exposes the runner home's Cargo binaries, bootstraps the repository-pinned toolchain when missing, and serializes toolchain installation across slots sharing that home. It leaves `RUSTC_WRAPPER` empty and skips sccache; hosted setup retains its existing cache behavior.

The lane selector includes the existing gate-registry audits in the own-test phase for production Rust changes. This catches uncoded refusals before later ring shards or the final train suite. Test-fixture-only changes avoid an additional library-test build; uncertain input still widens selection through the existing planner.

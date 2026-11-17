## Authoritative Sources

Factory-lane own-test builds run on the trusted local `cas-lane-light` runners. Their existing evidence upload includes `target/ci-impact/own-build.log`, which captures compiler output, test execution and watchdog resource heartbeats without masking command failures. Ring shards and browser jobs keep their existing runners and stage budgets.

The lane selector includes the existing gate-registry audits in the own-test phase for production Rust changes. This catches uncoded refusals before later ring shards or the final train suite. Test-fixture-only changes avoid an additional library-test build; uncertain input still widens selection through the existing planner.

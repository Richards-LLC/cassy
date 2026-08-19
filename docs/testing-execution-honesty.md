# Test-execution honesty

Cargo and nextest can exit zero when a requested filter matches no tests. A
green exit code is therefore not a test receipt. Any test result consumed by a
worker, CI lane, Make target, or release gate must run through
`scripts/run-verified-tests.sh` (or the stricter scoped wrapper).

The wrapper fails unless Cargo exits zero, prints at least one Cargo/nextest
harness summary, and reports a total passed count greater than zero. It
understands both Cargo test and nextest summaries, including ANSI-colored CI
output. `scripts/test-run-verified-tests.sh` replays green, zero-match Cargo,
zero-match nextest, swallowed-pre-harness, and real-failure receipts.

## Consumer inventory

| Consumer | Path | Zero-executed disposition |
| --- | --- | --- |
| Factory worker | `scripts/run-scoped-tests.sh` | Already guarded; also refuses an unscoped command and validates proof scope. |
| Factory hook | `pre_tool.rs` worker Bash gate | Direct `cargo test` and `cargo nextest run` are denied; workers must use the scoped receipt wrapper. `--no-run` remains allowed because it claims compilation only. |
| Local Make targets | `cas-cli/Makefile` test, test-full, test-verbose, test-%, clean-env, panic, test-nextest, integration, unit, dev-test, watch | Execution targets route through `run-verified-tests.sh`; test-scoped retains the stricter scoped wrapper. |
| CI scoped lane | `.github/workflows/ci.yml` | Already uses `run-scoped-tests.sh`. |
| CI full suite | `.github/workflows/ci.yml` shard step | Routes archived nextest shards through `run-verified-tests.sh`. |
| CI doctests | `.github/workflows/ci.yml` doctest step | Routes `cargo test --doc` through `run-verified-tests.sh`. |
| Release migration guard | `scripts/check-release-migration-snapshots.sh` | Routes component-output snapshots through `run-verified-tests.sh`. |
| Real-store test guard | `scripts/check-real-store-untouched.sh` | Routes its protected nextest invocation through `run-verified-tests.sh` before accepting an untouched-store result. |
| Compile-only / measurement commands | CI `cargo test --no-run`; benchmark `cargo test --no-run`; Cargo checks | Intentionally execute no tests and are labelled compile-only or build measurement; they are not test-pass consumers. |

`scripts/test-ci-test-tiers.sh` pins the CI and primary Make routing. The hook
unit tests pin the worker denial, and the release migration self-test exercises
the wrapper with a stubbed nextest receipt.

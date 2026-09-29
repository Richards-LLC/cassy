# Integration harnesses

Cargo links ten integration executables instead of one per source suite.
`autotests = false` in `cas-cli/Cargo.toml` makes the explicit `[[test]]`
entries authoritative. Source suites remain in `tests/` so their embedded
fixtures and `#[path]` modules keep the same source-relative locations.

| Target | Suites |
| --- | --- |
| `integration_contracts` | Builtin, agent, hook and instruction contracts |
| `integration_cli` | CLI, hub lifecycle, MCP protocol and host setup |
| `integration_cloud` | Cloud/team sync, storage, retrieval and search |
| `integration_factory` | Factory operations, worktrees and verification |

Six existing roots remain standalone: `builtin_archive_portability_test`,
`builtin_flavor_drift_test`, `component_output_test`, `e2e_test`,
`mcp_tools_test`, and `proptest_test`. These preserve scoped release-gate
selectors, snapshot filenames, or fixture modules addressed from the crate root.

Run grouped suites with nextest, which launches each test in its own process
and isolates changes to the environment and current directory. If using plain
`cargo test`, pass `-- --test-threads=1` for a grouped harness.

To select the old CLI suite:

```sh
cargo nextest run -p cas --test integration_cli cli_test::
```

The test's existing leaf name remains a substring filter. A complete old suite
now matches `-E 'test(cli_test::)'`; `binary(cli_test)` no longer selects it.
For a scoped proof, select the entire required harness so a narrower filter
cannot exclude a changed sibling suite. CI shards and nextest archives still
partition individual tests, including their new source-suite module prefixes.

When adding a suite, register its module in the appropriate harness and run:

```sh
python3 scripts/cas-test-targets.py cas-cli --check
```

The checker rejects missing or duplicate suites and more than ten harnesses.
It covers both `tests/*.rs` and Cargo's `tests/*/main.rs` entry points: 108
source suites, including `hooks_test/main.rs`, remain in ten harnesses.
Without `--check`, it prints `source_stem|cargo_target` mappings for selection
tools. The supervisor measures cold and warm
`cargo nextest run --workspace --no-run` before and after consolidation and
compares the executed test count at assembly; source inventory alone does not
prove runtime coverage or elapsed-time improvement.

Self-reexec helpers must strip the binary crate name from `module_path!()`
and retain the remaining suite path in their libtest `--exact` selector.
Check that the child reports exactly one executed test: a zero-match libtest
invocation also exits successfully.

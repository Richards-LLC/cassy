### Fixed — Factory tests

- **macOS test fixtures use physical temp paths** — on macOS, `/tmp` and
  `/var` are symlinks to `/private/...`. Four
  `factory_mcp_ops_test::test_gc_*` tests rooted their project or HOME at the
  raw temporary path, which host retention correctly refuses as a symlinked
  ancestor. Three `receipt_scope` related-test fixtures compared the fake
  runner's resolved paths with raw ones. These seven tests failed only on
  Macs. The fixtures now canonicalize their temporary roots, as
  `TestEnvGuard::temp_home` already does.

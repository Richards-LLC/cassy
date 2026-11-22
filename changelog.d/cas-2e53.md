### Changed — Test tooling

- **One physical temp-directory helper for macOS fixtures** — on macOS,
  `/tmp` and `/var` are symlinks to `/private/...`. Fixtures that compared a
  raw temp path with a resolved one each patched the mismatch at the
  assertion. `test_paths::physical_tempdir()` now roots those fixtures at the
  resolved temp directory once. `private_hub_tempdir` is built on it, and
  the earlier per-test fixes have moved onto it. The test-shape lint rejects
  a new or changed `.path().canonicalize()` in a Rust test and points to the
  helper.

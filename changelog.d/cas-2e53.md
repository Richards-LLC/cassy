### Changed — Test tooling

- **One physical temp-directory helper for macOS fixtures** — On macOS,
  `/tmp` and `/var` are symlinks to `/private/...`. Fixtures that compared a
  raw temp path with a resolved one each patched the mismatch at the
  assertion. `test_paths::physical_tempdir()` now roots those fixtures at the
  resolved temp directory once. `private_hub_tempdir` is built on it, and
  the earlier per-test fixes now use it.
- **Lane CI runs Rust tests under a symlinked TMPDIR** — Lane test phases
  point TMPDIR at a symlink to a real directory, so a fixture that compares a
  raw tempdir path with a resolved one fails in lane CI on Linux, not only on
  a macOS worker.
  Base attribution replays use the same symlinked TMPDIR, so such a
  failure is reported as caused by the delivery.

### Fixed — Factory

- **Orphan cleanup finds processes in a symlinked project** — The orphan
  scan matched each process's resolved working directory against the
  unresolved `.cas/worktrees` path. A project reached through a symlink
  reported no orphans. The scan now resolves those roots first.
- **Lane base attribution survives a cancelled run** — A cancelled lane
  run could leave its base worktree registered as missing but locked, and
  every later attribution on that runner failed to create it. The cleanup
  before each replay now clears a locked registration.

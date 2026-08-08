# CAS shell helpers

This directory is the canonical, tracked source for developer-oriented CAS
shell helpers. Install `cas-update` with:

```bash
contrib/shell-helpers/install.sh
```

That copies the helper to `~/.local/bin/cas-update` (override the destination
with `CAS_UPDATE_INSTALL_DIR`). The helper builds the current source checkout,
atomically installs `~/.local/bin/cas`, migrates and syncs local projects, then
turns over processes that still execute the exact pre-install binary bytes.

Plain `cas-update` performs the full workflow. `--no-restart` performs the
build/install/migrate/sync portion without signalling any runtime. Both
`--build-only` and `--sync-only` imply no runtime turnover. `--dry-run` is
strictly non-mutating and prints the frozen process plan.

Runtime turnover is deliberately conservative: it snapshots executable hashes
and Linux `/proc` start-time fingerprints before replacement, tries SIGTERM,
waits for a bounded grace period, and only then sends SIGKILL to the same exact
fingerprint. It never uses `pkill` or a process name as identity. Factory and
registered-server ownership is printed in the final summary; those processes
are not blindly relaunched because CAS does not currently expose a durable
non-MCP restart transaction for either registry. MCP-client-owned `cas serve`
processes reconnect through their client owner.

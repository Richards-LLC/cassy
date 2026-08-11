# Contributing to CAS

## Factory cloud client (disabled by default)

The factory daemon ships with a live-stream WebSocket client
(`cas-cli/src/ui/factory/daemon/cloud_client.rs`) that pushes factory state,
events, and pane output to a Phoenix-framework endpoint
(`/socket/websocket`). That endpoint is **not** implemented on the current
cloud backend (petra-stella-cloud is Next.js on Vercel, which can't host
long-lived Phoenix channels) and the feature it fronts — the Hetzner Slack
bridge / web terminal — is paused (see `project_claude_code_account_banned`).

The client is therefore gated behind a config flag and **disabled by
default**. Flip it on in `.cas/cloud.json`:

```json
{
  "endpoint": "https://your-phoenix-capable-host",
  "token": "…",
  "factory_cloud_client_enabled": true
}
```

Re-enable only when a Phoenix-capable backend is reachable. The REST-based
cloud syncer (`cas-cli/src/cloud/syncer/`) is independent of this flag and
always runs when logged in.

## Canonical install path

CAS must be installed to **one** location: `~/.local/bin/cas`. Any other
location (`/usr/local/bin`, `/usr/bin`, `~/.cargo/bin`) creates silent
duplicates: PATH-order changes (interactive zsh vs. a systemd service, or a
subagent invoking `cas` via absolute path) can promote a stale copy and
silently reintroduce fixed bugs.

- `scripts/cas-install.sh` installs to `~/.local/bin/cas` and warns about any
  other `cas` binaries it finds on PATH.
- On startup, `cas` itself scans PATH and emits a single-line stderr warning
  when duplicates with diverging mtimes are present. Silence it with
  `CAS_SUPPRESS_DUPLICATE_WARNING=1`, or force it on in non-TTY contexts with
  `CAS_WARN_DUPLICATES=1`. Hooks, `cas serve`, and `cas factory` are never
  warned.
- If you previously installed via `cargo install cas` or a distro package,
  remove those copies so only `~/.local/bin/cas` remains.

## Adding Features

**New CLI command**: Add variant to `Commands` enum in `cas-cli/src/cli/mod.rs`, create handler file in `cli/`. Prefer a dedicated integration test file at `tests/<feature>_test.rs` (e.g. `team_sync_test.rs`, `memory_share_test.rs`, `team_memories_e2e_test.rs`) over piling into `cli_test.rs` — isolated files surface regressions per-feature and keep compile times down. Shared fixtures (UUIDs, Cli/CloudConfig builders) go in `cas-cli/tests/common/mod.rs`; include via `mod common;` at the top of each test file.

**New MCP tool**: Add handler in `cas-cli/src/mcp/tools/core/` (data tools) or `cas-cli/src/mcp/tools/service/` (orchestration tools). Request types go in `cas-cli/src/mcp/tools/types/`. Register in the tool list via the `CasService` impl.

**New migration**: Create file in `cas-cli/src/migration/migrations/` following naming convention `m{NNN}_{table}_{description}.rs`. Add to the `MIGRATIONS` array in `migrations/mod.rs`. Each migration needs: unique sequential ID, up SQL, and a detect query. See `cas-cli/docs/MIGRATIONS.md` for full details. Migration ID ranges: Entries 1-50, Rules 51-70, Skills 71-90, Agents 91-110, Entities/Worktrees 111+, Verification 131+, Loops/Events 151+.

### cas-src close surfaces

Before claiming a change done, workers must add one pre-close task-note line for every applicable surface (and state `not applicable` for the rest): builtin skill/agent → Claude + Codex + Grok mirrors (`cas-8921`); MCP tool → CLI parity, docs, dispatch; hook/gate → `config_gen` + `.codex/hooks.json`; migration → bootstrap/reconciliation pins + `doctor_snapshot` (`cas-96f9`/m232); behavior contract → grep sibling old-contract tests (`cas-2327`/`cas-bc13`); state transition → reverse states; user-visible behavior → release-notes impact. This compact walk prevents a tested path from silently missing its sibling surfaces.

### Factory worker account selection

`coordination action=spawn_workers` accepts an optional `config_dir` for all
workers in the request. Claude workers use the tilde-expanded directory as
`CLAUDE_CONFIG_DIR`; an explicit parameter wins over the requesting
supervisor's own `CLAUDE_CONFIG_DIR`, which is captured when the request is
queued so the daemon cannot silently substitute its environment. With neither
value, spawning retains ordinary daemon-environment inheritance. Codex and
Grok workers ignore a resolved Claude directory and emit a warning. Only an
explicit `config_dir` removes `ANTHROPIC_API_KEY`, because that key overrides
Claude subscription OAuth; propagated supervisor settings retain existing API
key inheritance.

## Testing

Integration tests are in `cas-cli/tests/`. Key test files:
- `cli_test.rs` — CLI command integration tests
- `mcp_tools_test.rs` — MCP tool handler tests
- `mcp_protocol_test.rs` — MCP protocol compliance
- `factory_server_test.rs` — Factory WebSocket server tests
- `distributed_factory_test.rs` — Multi-agent factory tests
- `proptest_test.rs` — Property-based tests
- `e2e_test.rs` / `e2e/` — End-to-end tests
- `team_sync_test.rs` — `cas cloud sync` team-queue drain path
- `memory_share_test.rs` — `cas memory share|unshare` CLI behavior
- `team_memories_e2e_test.rs` — end-to-end team-memories flow (share → push → pull)

Dev dependencies include: `insta` (snapshot testing), `wiremock` (HTTP mocking), `rstest` (parametrized tests), `proptest` (property-based), `criterion` (benchmarks), `cas-tui-test` (TUI testing).

## Skill & Rule Sync

CAS auto-syncs rules to `.claude/rules/` and skills to `.claude/skills/` as SKILL.md files with YAML frontmatter. The sync logic lives in `cas-cli/src/sync/`. Rule promotion: Draft -> Proven via `mcp__cas__rule action=helpful`.

### Builtin skill references

Files under `cas-cli/src/builtins/**/references/` are owned by their skill and synced with a baseline ledger: a destination that differs from both the recorded baseline and every version CAS has shipped is preserved as a local customization (and surfaced in a SessionStart banner). The set of "versions CAS has shipped" is the embedded `cas-cli/src/builtins/reference-history.json`.

**After changing any builtin reference file — and before cutting a release — run:**

```bash
./scripts/gen-builtin-reference-history.sh
```

and commit the regenerated JSON. Skipping it means the version you just replaced is not recognized as CAS content downstream, so installs that still hold it will keep it forever instead of upgrading (cas-0c0a).

## Releasing

### Version policy

- `cas-cli/Cargo.toml` version is the release version (currently 2.0.0).
- Internal crates (`cas-core`, `cas-mux`, `cas-mcp-proxy`, etc.) stay at `0.1.0` unless published separately.
- **Patch** (x.y.Z): Bug fixes, doc updates, performance improvements.
- **Minor** (x.Y.0): New features, new CLI commands, new MCP tools.
- **Major** (X.0.0): Breaking changes — cloud protocol changes, CLI flag removals, MCP tool schema changes.

### Breaking changes

These require a major version bump:
- Cloud sync protocol changes (push/pull shape, endpoint paths)
- CLI flag or subcommand removals/renames
- MCP tool parameter schema changes (field renames, type changes)
- Migration format changes that break older DBs without a migration path

### Steps to cut a release

1. Update version in `cas-cli/Cargo.toml`.
2. Add a `## [X.Y.Z] - YYYY-MM-DD` section to `CHANGELOG.md` (Keep a Changelog format).
3. Update the comparison links at the bottom of `CHANGELOG.md`.
4. Commit: `chore(release): bump to vX.Y.Z`.
5. Before creating a tag, run the release migration-snapshot guard:

   ```bash
   ./scripts/check-release-migration-snapshots.sh
   ```

   When `cas-cli/src/migration/migrations/mod.rs` changed since the last tag,
   this runs the required command `cargo test -p cas --test component_output_test`.
   That snapshot suite checks the doctor/status schema and ledger counts that a
   migration moves; the scoped release suites do not build it. If no previous
   tag is reachable, the guard runs the snapshots conservatively.
6. Prefer `./scripts/release.sh --publish` for the local release path. It runs
   the same guard before it can create or push a tag, so the check is enforced
   even if the checklist step is missed.
7. Create an annotated tag, then run the fast release preflight **before pushing it**:

   ```bash
   git tag -a vX.Y.Z -m "vX.Y.Z"
   ./scripts/check-release-preflight.sh vX.Y.Z
   ```

   This rejects a dirty tree, a lightweight/stale tag, mismatched release-train
   crate versions, a missing changelog heading, or lockfile drift before the
   expensive release builds begin. `release.sh` runs the same guard automatically
   for non-`--build-only` releases.
8. Push: `git push && git push --tags`.
9. Create GitHub release: `gh release create vX.Y.Z --generate-notes`.

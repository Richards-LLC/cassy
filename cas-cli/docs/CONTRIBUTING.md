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
   this runs the required command
   `cargo nextest run -p cas --test component_output_test`.
   That snapshot suite checks the doctor/status schema and ledger counts that a
   migration moves; the scoped release suites do not build it. If no previous
   tag is reachable, the guard runs the snapshots conservatively.
6. Run `./scripts/release.sh` to produce local audit evidence without touching
   the remote. The tag-triggered GitHub Release workflow—not the local
   `dist/local-audit/` archives—creates the normal release. A local archive is
   evidence that the tagged source builds; it is never evidence of the shipped
   bytes or an announcement digest. The emergency
   `--publish-tag --manual-publish --acknowledge-workflow-conflict` path is
   only for a disabled/unavailable workflow and still requires the published
   receipt in step 9 before any digest is announced.
7. Create an annotated tag, then run the fast release preflight **before pushing it**:

   ```bash
   git tag -a vX.Y.Z -m "vX.Y.Z"
   ./scripts/check-release-preflight.sh --local vX.Y.Z
   ```

   This rejects a dirty tree, a lightweight/stale tag, mismatched release-train
   crate versions, a missing changelog heading, or lockfile drift before the
   expensive release builds begin. `release.sh` runs the same `--local` guard
   before its local audit and tag push.

   `--local` inspects the local tag object. Omit it only on the CI side, where
   the tag is already pushed: the default lane re-fetches the exact remote tag
   object first, so `actions/checkout` handing back a peeled ref cannot turn the
   annotated-tag check into a check of checkout's local ref shape. Running the
   default lane before the push always fails with
   `couldn't find remote ref refs/tags/vX.Y.Z`.
8. After reviewing the audit, `release.sh --publish-tag` pushes the tag and
   starts the workflow. Publishing is deliberately explicit: a bare
   `release.sh` invocation never touches the remote. It builds Linux on its
   host and, on macOS, also builds the Darwin audit target; this host-dependent
   audit coverage does not change what ships. CI always builds and publishes
   both Linux x86_64 and macOS ARM64 assets.
9. Wait for the workflow-created release to be published, then derive every
   announcement digest from freshly downloaded published bytes:

   ```bash
   ./scripts/release-published-receipt.sh vX.Y.Z
   ```

   The command fails closed while the release object is draft, either required
   asset is still uploading, or a downloaded byte hash disagrees with GitHub.
   Copy its emitted fields into the release-note draft; never transcribe a
   digest from `dist/local-audit/`.

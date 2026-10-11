# Contributing to Cassy

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

### Team-only project sync

Set `[cloud] team_only = true` in a project's `.cas/config.toml` after linking
that project to a team with `cas cloud team set`.
The registered `cas config set cloud.team_only true` command sets the same key;
`cas config get cloud.team_only` and reset/list use that configuration too.
The default is `false`:
team-eligible project tasks, dependencies, memories, rules, and skills keep
their existing personal plus team queue behavior. With the opt-in enabled,
those project rows use only the team queue. Global rows and private memories
remain in the personal queue. `cas cloud push` and `cas cloud sync` avoid a
personal project push, and queued personal copies of project rows are removed
locally before sync. Cloud rows are never deleted by this cleanup. Without
an active team, sync refuses and `cas doctor` reports an error.
The personal push API requires a project identity, so global and private rows
from a team-only root remain queued rather than re-registering its retired
personal project. `cas cloud status` and `cas doctor` show the held count.

Standalone push/pull, the daemon cycle, and MCP startup use the same team-only
scope policy. To remove stale personal outbox rows rejected by the server with
`team_owned_project`, run `cas cloud queue --purge-team-owned`. It removes only
personal rows with that exact structured rejection, keeps local entities and
team queue rows, and makes no cloud calls. `--json` reports the purged count,
scope and reason. This cleanup does not backfill or delete remote cloud data.

`duplicate_of_other_project` means the same row already lives under another
Cloud project identity. The rejection names that owning project. If both
identities represent the same project, ask the Cloud owner to register and fold
the alias first, then run `cas cloud project --adopt-aliases`,
`cas cloud queue --retry --retry-reason duplicate_of_other_project`, and
`cas cloud sync`. Otherwise, retire the local duplicate. Retrying alone cannot
repair remote ownership; alias adoption rewrites local task provenance and
does not move Cloud rows.

## Canonical install path

Cassy must be installed to **one** location: `~/.local/bin/cas`. Any other
location (`/usr/local/bin`, `/usr/bin`, `~/.cargo/bin`) creates silent
duplicates: PATH-order changes (interactive zsh vs. a systemd service, or a
subagent invoking `cas` via absolute path) can promote a stale copy and
silently reintroduce fixed bugs.

- `scripts/cas-install.sh` installs to `~/.local/bin/cas` and warns about any
  other `cas` binaries it finds on PATH. It also offers to wire that directory
  onto PATH in the **login** shell's startup file (zsh `.zshenv`, bash
  `.bashrc`/`.profile`), guarded by `# >>> cassy path >>>` markers so a re-run
  is a no-op, and verifies the result with `cas --version` in a fresh login
  shell before claiming the install succeeded. `CAS_WIRE_PATH=1|0` overrides the
  prompt; the rc-edit seam is covered by `scripts/test-cas-install.sh`.
  Before extraction, it verifies the archive against the selected asset's
  SHA-256 in GitHub Release metadata; missing or mismatched receipts fail
  closed while preserving the installed binary. This detects corruption but
  is not an independent signature: GitHub/repository release authority serves
  both bytes and digest, and no separate Cassy signing trust root is named.
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

### Code index ownership across worktrees

Code-file rows retain their normalized absolute source paths. Reconciliation
only retires absent paths owned by the checkout it scanned; sibling and nested
linked checkouts keep their rows. Retirement and scan receipts derive authority
from configured scan roots; recursive watcher events cannot add a nested checkout.
A full reconciliation uses a fresh scan of those roots. An explicitly configured
nested checkout retains its own reconciliation scope. A configured subdirectory
can retire its own absent files; it preserves the full-checkout scan receipt
until a full checkout root is visited. Paths without an identifiable checkout
remain untouched rather than being treated as another checkout's deletions.

Code scan receipts use `worktree:<canonical checkout root>` in the existing
`code_index_state.repository` TEXT key. Historical repository-name receipts
remain stored but do not certify checkout coverage or HEAD. `cas doctor` and
`cas status` count current eligible, decodable source files from disk. The
manual index command and daemon select the current linked checkout when its
Git common directory matches the explicit store's repository.

A busy BM25 writer defers the remaining retirement sweep after one bounded wait;
source rows remain its retry manifest. `cas index code --json` reports
`files_deferred`, separately from errors, and the daemon schedules a fresh
reconciliation without requiring another filesystem event. Doctor autofix keeps
its warning while retirements are deferred and supplies `cas index code` to retry;
it reports the symbol index fixed only after deferred work and errors are clear.

### Worker browser and JS memory admission

Worker browser/Vitest suites, npm test/build/typecheck/journey scripts and
known Node suite entry points run through
`python3 scripts/worker-memory.py -- <command>`. The worker PreToolUse hook
routes those commands automatically when the checkout has that helper.
Without it the hook warns and retains the existing permission guards.
Plain npm reads, inline Node code and arbitrary Node scripts retain their
existing permission decisions. The hub-web build, typecheck, visual-QA
and verified test entry points also acquire admission when run directly.
The helper owns the command's process group and ends it when the command
returns, so it refuses a routed command that backgrounds a job with `&`:
the suite's receipt launcher would die while a detached runner lived on
unreported. Run a long suite in the foreground of a persistent session with
its output redirected to a log, and read the log.

Admission uses `assembly-proof.py`'s fresh host memory snapshot and reserve
(default: greater of 8 GiB or 25% of physical RAM). A suite needs an assumed
4 GiB for browser/unknown commands, 1 GiB for tsc/Vite builds or 2 GiB for
capped Vitest, plus 2 GiB headroom. Concurrent suites take weighted FD-locked
slots while the fresh budget covers all live reservations and headroom. Literal
shell and npm scripts are classified from their actual commands; unknown or
expanding scripts keep the browser estimate. Smaller commands can use remaining
capacity while a browser waits. Proofs take priority and hold exclusive intent
around all assembly producers and consumers; worker commands wait until those
proofs finish. Legacy checkout budget leases still exclude new admissions.
The linker pool remains separate.
Nested commands reuse admission only while its private claim, live lock and
process ancestry validate; setting an environment flag does not waive it.

Waits print `waiting for host memory (proof running), N s` and memory samples.
`CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS` bounds waiting (default 600 s),
with `CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLL_SECS` resampling (default 1 s).
Expiry fails before starting the suite. A running worker command is terminated
with its own process group if fresh budget falls inside the 2 GiB headroom.
These are admission estimates and sampled protection, not OS memory limits;
other applications remain outside this cooperative protocol.

Verified frontend tests default to one Playwright worker, honour explicit requests
up to four, and enforce two Vitest workers. A proof's own child script tests reuse
its admitted budget rather than waiting on themselves. Use `TMPDIR` on the
approved scratch volume for fixture staging. Script-level admission tests need
no browsers or Cargo: `python3 scripts/test-worker-memory.py`.

### cas-src close surfaces

Before claiming a change done, workers must add one pre-close task-note line for every applicable surface (and state `not applicable` for the rest): builtin skill/agent → Claude + Codex + Grok mirrors (`cas-8921`); MCP tool → CLI parity, docs, dispatch; hook/gate → `config_gen` + `.codex/hooks.json`; migration → bootstrap/reconciliation pins + `doctor_snapshot` (`cas-96f9`/m232); behavior contract → grep sibling old-contract tests (`cas-2327`/`cas-bc13`); state transition → reverse states; user-visible behavior → release-notes impact. This compact walk prevents a tested path from silently missing its sibling surfaces.

### Evidence-only task close

Successful QA reports, ledgers and release drafts intentionally retained outside
integration can close through the MCP task tool with `action=close`,
`evidence_only=true`, `evidence_only_artifact_path=<existing durable evidence>`,
`evidence_only_reference=<PR URL or branch:factory/name>`, and a non-empty
`reason`. Only a live registered supervisor can authorize this disposition.
There is no CLI task-lifecycle command; the unified MCP task tool owns dispatch.

CAS measures the recorded delivery anchor (or the assigned worker branch before
parking) against the fresh integration target. Every unmerged commit must touch
only regular, non-executable evidence formats under `docs/` or `artifacts/`:
Markdown, text, HTML, PDF, SVG, images, JSON, CSV/TSV, logs or YAML. Source changes,
code renamed into docs, reverted code, symlinks and submodules are refused.
Measurement is bounded to 256 commits and fails closed when Git cannot prove it.
The artifact must exist beneath this project's configured task artifacts directory;
local paths and secret-shaped values are forbidden in the portable PR/branch reference.

The successful `evidence_only` terminal outcome counts completed report delivery,
requires no parent-epic code integration, and logs supervisor identity, rationale,
base SHA, delivery SHA and measured paths. Reopening clears the structured receipt.
It cannot combine with negative-result, completion or external-verification receipts,
or close a Gate/Epic. Ordinary closes keep the merge gate; measured negative
experiments retain their separate `negative_result=true` outcome.

### Factory worker MCP and credential access

A supervisor can waive independent QA before a delivery parks with
`verification action=qa_waive task_id=<id> head_sha=<full pushed SHA> summary="<reason>"`.
The supplied SHA must equal the live origin tip of the task's factory branch;
the waiver covers only that commit. Without `head_sha`, the existing recorded
delivery and rebase rules choose the binding. This operation is MCP-only.

Declare resources that stay on the supervisor in `.cas/config.toml`:

```toml
[factory]
supervisor_only_mcp = ["vercel", "neon"]
supervisor_only_env = ["VERCEL_TOKEN", "NEON_API_KEY"]
```

Both lists default to empty and appear in `cas config list`. Server names are
exact. These denials override project proxy credential grants and read-only
server declarations for every worker harness. The supervisor's configuration
and environment are preserved. Spawn diagnostics record denied names only.

Configured workers get a private, materialized MCP configuration under
`<cas_root>/worker-mcp/<name>.json`. Tracked and existing worktree `.mcp.json`
files stay unchanged, so provisioning keeps the tree clean and cannot commit
supervisor-only removals into the project. Claude
uses `--strict-mcp-config` so local and user scopes cannot add servers; allowed
direct servers must be declared in the project file. Cassy remains available
even if it was registered only in local scope. Every worker uses the same private
store path, including shared-cwd workers. Codex disables the named native MCP servers, and
its Cassy proxy filters those upstreams before startup and reload.
Codex addresses dotted or otherwise special server names with a quoted key in
a parent-table override so the name remains literal.
Worker snapshots cannot overwrite the supervisor's shared proxy catalog/health.
Listed environment names are removed from inherited and explicitly granted
values, including machine credential bootstrap and retained proxy stdio
servers' explicit environment maps, before credential resolution and reload.
Codex workers remain spawnable with environment restrictions. Before launch,
Cassy parses the selected user Codex home (`CODEX_HOME` or `~/.codex`) and the
project `.codex/config.toml` files along the launch directory's ancestors,
without running native inventory or discovery. A server whose literal `env`
table contains any denied name is disabled for that worker. Keys retained
from any parsed layer count, even if a later layer changes their values.
Other native servers remain enabled, and the supervisor's files stay unchanged.

This inspection covers parsed TOML, not expanded plugin or runtime
contributions. Unknown plugin/layer contributions and unreadable or malformed
Codex files produce one warning naming the unevaluated sources; the worker
still spawns. Review those contributions before granting credentials. Process
environment removal and the explicit supervisor-only server overrides still
apply. This bounded policy deliberately preserves the standard Codex worker
lane; it does not promise complete isolation of unknown native contributions.
Grok and other native harnesses without supported per-launch MCP/environment
isolation refuse a worker launch when either list is nonempty. They retain
ordinary native discovery with empty lists; the supervisor remains unrestricted.
The refusal does not select a different harness or provider. Invalid
configuration or a failed materialization also refuses the worker launch.

Deployment and production operations needing denied resources run through the
supervisor. Workers never source an interactive shell to obtain operator
credentials. Existing explicit project grants and the operator-provisioned
read-only GitHub token remain available unless denied here; this policy adds
no credentials to any harness.

### Claude workspace trust and the hook canary

Claude Code runs no hooks (SessionStart, PreToolUse, from any settings source)
in a workspace it has not trusted, and factory agents launch with `IS_DEMO=true`,
which skips the trust dialog without trusting. Before cas-0f5b every Claude
worker therefore ran unguarded: no capped cargo runner, no worker-memory
admission, no Slack, publication or browser guards. Two pieces now close that:

- **Trust at spawn.** The Claude backend's `prepare_workdir` merges only
  `projects["<cwd>"].hasTrustDialogAccepted = true` into the agent's
  `$CLAUDE_CONFIG_DIR/.claude.json` (or `~/.claude.json`), under the shared
  `.claude.json.cas-lock`, via temp file, fsync and rename, then re-reads it
  and retries once (live sessions rewrite the file). A config it cannot parse
  is never rewritten; the launch is refused instead. The supervisor's own
  checkout is trusted the same way.
- **The canary refuses.** A factory agent's SessionStart hook writes
  `.cas/factory/hook-canary/<agent>.json`. Spawn verification (including
  respawn and recycle) kills a Claude worker whose marker has not appeared
  within 60 seconds of launch, marks it crashed and tells the supervisor.
  There is no silent degrade.

Rollout: after this merges, respawn every live Claude worker. Hooks start
denying things those workers never saw before: raw `cargo`, `&` background
jobs under worker-memory admission, non-Violet Slack writes, publication and
unfiltered browser runs. Codex workers are unaffected (their trust was
already pre-seeded by `codex_trust.rs`).

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

For CLI chores that need operator credentials, explicitly grant environment
names in the project's `.cas/config.toml`:

```toml
[factory]
worker_credential_env = ["GITHUB_TOKEN", "VERCEL_TOKEN"]
```

This list defaults to empty, appears in `cas config list`, and applies equally
to Claude and Codex workers. Only listed names override the default removal of
protected operator tokens. Values come from the operator environment or the
existing private credentials-file/login-profile reader; Cassy never executes
an interactive shell. Configure these grants only when workers should perform
the associated operations. Factory identity variables (`CAS_*`, except
protected credential names such as `CAS_CLOUD_TOKEN`) cannot be granted.

Missing names generate one names-only warning in the worker spawn receipt and
the worker still starts. A name also listed in `supervisor_only_env` stays
denied, with the conflict named in the same warning. An operation needing an
unavailable credential then fails visibly; one missing token does not stop the
fleet. Supervisors keep their own credentials and configuration.

## Testing

### Duplicate-task warnings

Task creation excludes common planning words and prose such as `NOT` and
`before/after` from distinctive identifiers. Generic-only title overlaps
require near identity; exact duplicate titles and concrete code/path overlap
still warn. Intentional duplicates retain the `confirm_warning=true` escape.

### Task lease release

`task action=release` lets a live registered supervisor release a worker's
lease and records the supervisor identity in lease history. Other callers
may release only their own lease; `force=true` does not grant that authority.
Releasing an InProgress task returns it to Open and clears its assignee.
AwaitingMerge retains its delivery state, including when no active lease remains.

### Branch-only target correction

`task action=update target_branch=<branch>` preserves the task's repository
binding or defaults a legacy targetless task to the current project repository.
The corrected branch is validated, so a deleted old epic branch can be repaired.
The same default applies to supervisor `proof_scope_fix=true` corrections.
An unchanged correction leaves the proof cycle intact and reports the last
recorded close rejection for that task, including pre-close hook failures.
Retry `task action=close` to refresh the current gate before correcting scope
again; historical diagnostics do not replace a fresh close attempt.

### Server-list MCP contract

`factory action=server_list` reports verified running servers by default.
Use `status=stopped`, `status=dead`, `status=unverified`, or `status=all` to
inspect history or entries whose process identity cannot be verified.
`task_id` filters by exact owning task; `owner` accepts an exact worker name
or registered agent ID. Filters combine. `limit` defaults to 20, must be
positive, and is capped at 50. Each server occupies one line of at most 512
bytes; long fields and port lists are abbreviated. A truncation notice gives
the number of matching entries. The legacy coordination route uses the same
filters and limits. Regression coverage lives in `server_registry_mcp_test`
and the service's `server_ops_tests`; no new CLI command is introduced.

### Task branch adoption

For an inherited factory delivery, use `task action=transfer id=<task-id>
to_agent=<worker> adopt_branch=true` (add `supervisor_override=true` when a
supervisor transfers another worker's active lease). Cassy copies the task's
recorded delivery tip to `factory/<receiver>-<task-id>` in the receiver's
registered worktree, then updates its assignment and delivery anchor. Commit
there; the isolation guard still rejects the old owner on that branch.

The receiver must have a clean isolated worktree in the task's repository,
with HEAD able to fast-forward to the delivery. Dirty, divergent and foreign
checkouts are refused before assignment or lease changes. The original branch
is preserved as handoff history. Omitting `adopt_branch` keeps the existing
assignment-only transfer behavior.

### MCP mutation timeout receipts

The MCP response budget is 55 seconds; timeout diagnostics report the measured
elapsed time and budget separately. Message enqueues and task writes carry
request-scoped commit evidence. A timeout after an observed commit reports
`COMMITTED`; message receipts include `notification_id`. `UNKNOWN` means this
request's commit was not confirmed. Re-query state before retrying either case.
The error's structured data includes `mutation_outcome`, `notification_id`,
`elapsed_ms` and `budget_ms`.

Optional resource notifications and factory recall have bounded response waits.
Late recall output is retained for the next successful response, including mail
already consumed by the background reader. Message delivery remains asynchronous;
use `coordination action=message_status` to inspect handoff and recipient evidence.

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
- `artifact_publish_test.rs` — `cas artifact publish` path guard, size ceiling, and the assertion that a signed upload URL never reaches disk or stdout
- `credential_debug_guard_test.rs` — repo-wide guard: no struct holding a token, API key or pre-signed URL may derive `Debug` (a derived one prints the credential verbatim). Add a redacting `impl fmt::Debug` plus a test rather than an allowlist entry.

### Terminal output gate

Anything a command prints for a person (`cas doctor`, `cas update`, `cas factory status`, a
table, a progress line, an error) is designed under the `cas-cli-craft` builtin skill and gated by
`scripts/terminal-qa.mjs`: it runs the command in a pty at 80 and 120 columns on the dark, light
and both Solarized palettes plus piped, `NO_COLOR` and `LC_ALL=C` runs, and fails on wrapped rows,
split tokens, colour under 3:1 (marks) or 4.5:1 (text), truncation with no escape flag, glyphs on a
C locale, SGR under `NO_COLOR`, redraws in a pipe, and a `--json` stream that is not one document.
BSD/macOS and util-linux `script` are supported. Empty captures fail; unavailable runners
exit 2 without retaining a PASS receipt. Diagnostic stderr is captured and checked, while
JSON stdout stays separate. Expected nonzero command exits do not fail rendering QA.

```bash
node scripts/terminal-qa.mjs --label cas-doctor --escape-flag --verbose --json-flag --json -- cas doctor
node --test scripts/terminal-qa.test.mjs   # the gate's own suite (planted-defect fixtures)
```

Paste the `terminal-qa: PASS …` receipt line into the pre-close note; briefs and captures for the
shipped commands live in `docs/design/cli/`.

Dev dependencies include: `insta` (snapshot testing), `wiremock` (HTTP mocking), `rstest` (parametrized tests), `proptest` (property-based), `criterion` (benchmarks), `cas-tui-test` (TUI testing).

## Build, assembly and CI policy

This section moved here from the repository `CLAUDE.md`, where it loaded into every session.

### Commands

```bash
# Supervisor/operator only — factory workers never run these
cargo build                          # Dev build
cargo build --release                # Release build (LTO, strip)
cargo build --profile release-fast   # Fast release (thin LTO, 16 codegen units)
cargo check -p cas --lib --tests     # Compile feedback, no test linking/runs
scripts/run-scoped-tests.sh -p cas --lib module_name
scripts/run-scoped-tests.sh -p cas --test integration_cli cli_test::
cargo nextest run -p cas             # Full suite: epic assembly and release gates
cargo test -p cas --doc              # Doctests (nextest does not support them)
cargo bench --bench code_indexing    # Benchmarks
make test-release-panic              # Verify A2/A3/B3 panic isolation under release profiles
```

The integration suites link through ten explicit Cargo harnesses. Use the
source suite's module prefix to select its tests; the six standalone release
and fixture targets keep their names. See [integration harnesses](../tests/integration/README.md)
for the inventory and per-test process isolation rules. Run
`python3 scripts/cas-test-targets.py cas-cli --check` from the repository root
after adding an integration suite so a missing module cannot silently drop tests.

Install the standard local runner once with `cargo install cargo-nextest` (or
`make -C cas-cli install-tools`). `scripts/run-scoped-tests.sh` defaults to
nextest and rejects a silent zero-test success.

### Worker checks, targeted tests and supervisor assembly

Workers may type-check their committed change with exactly
`cargo check -p <affected crate> [-p <crate> ...] --lib` for lib-only edits or
`--tests` when test files changed; choose one target flag. Include consumers of
changed shared interfaces. The PreToolUse guard routes that command through
`cas factory worker-check`, which holds an OS builder-slot lock until Cargo
exits. The route covers Claude's Bash and Codex's `exec_command`, including
calls made from code mode (`functions.exec`). Codex applies a hook's rewritten
input only with `permissionDecision: "allow"`, so the hook emits that for Codex
(cas-980d). Lock descriptors are close-on-exec so compiler-cache daemons cannot
retain slots after the runner exits. It checks the existing build guard, and enforces `max_concurrent_builders`
even across simultaneous launches. A refusal requires retrying later. Run long
checks in the background with a log. Compile checks execute no tests.

The runner requires a clean committed worktree, forces its private seeded
`target/`, and records `check: PASS <sha>` with the selected packages. Task close
copies matching exact-delivery receipts into worker evidence. Dirty trees,
failed retries, other worktrees and other SHAs cannot supply this receipt.
Check receipts are optional compile-only evidence and do not waive test proof.

Shell redirection opens logs before the runner checks that commit. Use a
Git-ignored `target/worker-check.log` or the task's artifacts directory; create
the parent directory first. In-repo logs that Git does not ignore are refused
by the hook. The clean-head gate still rejects source changes.

Rust-touching supervisor lane merges require separate combined-tree evidence.
Run `python3 scripts/check-lane-compile.py . <target> <source> --prove` as a
supervisor before merging, backgrounded with a log. It uses the same capped
runner for package-scoped `--lib` and `--tests` checks in a private preview and
records PASS only for the resulting tree. Lane-tip receipts do not qualify.
The preview preflight, `release-train.sh --check-lane`, and the actual detached
Git merge enforce this evidence before the epic ref advances. Docs/scripts-only
merges need no compile receipt. Rust merges require clean linked target checkouts.

The no-build lane preview uses a load-aware wall budget for fast rows: 60 seconds
times `(1 + one-minute load / CPU count)`, capped at 600 seconds. At load of
1–1.5 times the core count, this allows 120–150 seconds. An exhausted budget
reports unfinished rows from the gate's plan and timing receipts, plus a retry
command. Supervisors can rerun
`python3 scripts/check-lane-fast-rows.py . <target> <source> --timeout-secs 180`
with an explicit bound of 1–1800 seconds. The preview preserves branch refs and
cleans its temporary checkout on success, row failure, and timeout.

Workers may also run exactly
`cargo nextest run -p <crate> [--lib|--test <harness>] -E 'test(module::name)'`.
An omitted target selects `--lib`; `--test` must name one explicit harness from
`scripts/cas-test-targets.py`. Select exactly one package and a mandatory
positive named-test filter (`test(name)` or `test(=name)`, joined by `|`/`&`).
Empty/all(), regex/glob, negated and binary-wide selectors are refused, as are
environment prefixes, compound commands, repeated packages and broader flags.
The same capped runner invokes the shared `run-verified-tests.sh` zero-test
guard, forces the private target, and records only successful nonzero counts
against an unchanged clean commit: `test: PASS <sha> <package> <filter> <count>`.
Failed retries invalidate that scope's receipt; close imports all matching
exact-delivery receipts. Targeted receipts complement full-suite assembly proof.

The supervisor runs the full build and Rust suite once per release candidate at
assembly it runs `python3 scripts/assembly-proof.py prove <epic-worktree>` and
records `ASSEMBLY_PROOF: head=<epic tip sha> result=PASS command=<cmd>
log=<path>` on the epic. Workers still cannot run build/test/clippy/run or unfiltered nextest,
`rustc`, scoped-test scripts, or `make test*`. Child closes reference assembly
proof rather than scoped `--proof` or `loaded_proof` notes. That holds for a
worker's close and for a supervisor closing an epic child whose delivery is
already on the integration branch: close records a pending-assembly note when
the epic has no covering `ASSEMBLY_PROOF` yet. Standalone tasks still carry
their own receipts. A scoped receipt may be the runner's `SCOPED_PROOF:
targets=…` or `command=…` line, or, when that exceeds the task-note limit, the
short `SCOPED_PROOF_RECEIPT: id=… path=…` line; `--proof` always writes that
file (default `<git-common-dir>/cas/scoped-proof/<head>.receipt`) and close
verifies its digest. Non-Rust suites are unaffected. An older runtime that denies the check exception requires parking
with the unverified crates and test filters named for assembly.

The assembly command runs the gate's `ci-script-tests` row:
`make -C cas-cli test-ci-tiers`, with factory identity and inherited make
dry-run/ignore-error modes removed. A failing script suite retains its output
and stops assembly before either Rust suite. The full release gate runs this
same mandatory row before build and Rust test rows. The short `--fast-rows`
lane checks run only `ci-script-tests-changed`: the tier entries whose script
or subject (`scripts/<stem>.*` for `scripts/test-<stem>.*`) the lane changed.
Entries that take over 20 seconds alone are reported as deferred to the full
row.

The fast rows also run the no-build checks that the October 2026 release cut
found only at assembly.
Each runs when the lane changes its inputs, and the full gate always runs it:

- `journey-catalog`: `journeys-for-diff.py --check`, so every catalog step
  still matches a `test.step` title in its journey spec.
- `builtin-skill-limits`: skill description, size and line limits checked on
  the exact `include_str!` catalog bytes, mirroring the Rust tests.
- `doctor-snapshot`: every doctor snapshot row sits in the group
  `CheckGroup::for_name` assigns, and a lane that adds a
  `recorder.mark(..)` phase also updates the snapshot.
- `migration-registry`: every migration file is declared, registered and
  listed in id order.

Contract phrases were already part of `builtin-doc-hygiene`. On this host a
lane spanning that release's three epics ran every fast row in about 25 seconds.

Release scripts used from macOS source `scripts/release-portable.sh` for
timestamp parsing and canonical paths. The timestamp helper retains GNU date
results on Linux and falls back to Python for timezone-qualified ISO values
on BSD hosts, including offsets and fractional seconds. The path helper
resolves symlinks and missing trailing components without GNU `realpath -m`
or `readlink -f`. Receipt paths and their worktree use the same canonical
form before containment checks. `scripts/test-release-portable.py` exercises
the report stage with GNU date and path commands unavailable. Provisioning
and persistent Actions cache maintenance remain Linux-only and explicitly
require GNU tools and Linux kernel interfaces.

When memory permits, the script tier overlaps the native nextest precompile
and archive producer compile. Both producer phases finish and the script tier
passes before native full-workspace tests run, followed by the archive consumer.
The two test consumers remain sequential: nextest groups apply within one
process, and host ports and hub processes may be shared between suites. Native
and clone builds use separate Cargo targets; each row has separate logs and
gate scratch directories. The plain clone and archive remap remain outside
disposable roots and every `.cas` ancestor. Archive extraction stays on disk
with `--extract-to`; only disposable test temp directories and fixture HOMEs
use the native temp filesystem. Missing-wrapper, empty Cargo home, reduced
PATH and the component-output snapshot exclusion remain archive requirements.

Assembly reads Linux `MemAvailable`, or macOS `hw.memsize` and `vm_stat`
free/inactive/speculative pages, before admitting producers and each consumer.
`CAS_RELEASE_GATE_ASSEMBLY_BUILD_JOBS` sets a per-producer job ceiling; the
default shares available cores equally, and memory may lower it further.
`CAS_RELEASE_GATE_ASSEMBLY_RESERVE_GIB` overrides the reserve; its default is
the larger of 25% of physical RAM and 8 GiB. Both knobs accept positive integers.
The producer budget uses 8 GiB for the large cas compile/link unit, rounded up
from soundwave's measured 7,293,348 KiB maximum RSS (serial proof `7e4c6f50`,
head `abd6817b5`), plus an assumed 256 MiB per dependency job and 2 GiB for
scripts. A shared host/user linker pool bounds both producers, budgeted at
2.1 GiB per link. Soundwave's 2026-10-05 incremental relink sampler
(`.cas/perf-98a0/link-rss.log`, 0.5s samples) measured 2,190,228 KiB maximum
`ld.mold` RSS (2.089 GiB), with `rustc` peaking at 4,775,752 KiB (4.555 GiB).
The cold-proof 8 GiB producer bound remains because incremental code generation
does not establish the cold peak. Link admission rechecks memory while holding
an atomic admission lock. The live slot count is
`clamp(floor((MemAvailable - reserve - 2 GiB) / 2.1 GiB), 1, maximum)`,
where `CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS` sets the maximum (default 8,
positive integer). A count of 1 still waits if one link cannot fit. All active
leases count, including higher slots after memory shrinks or a different cap
is chosen. Queued children reserve a full estimate before starting; the pool
is host/user-wide under `/var/tmp`, independent of producer `TMPDIR`.
Every memory-sampled attempt records capacity and occupancy; every admission
records its slot. `execution.link_jobs` is the configured maximum, while
`link_slots` in each admission is the fresh capacity. Each invocation records
`peak_waited_driver_rss_bytes` in `link-rss.jsonl`: `wait4` RSS for the exact
waited driver, excluding unwaited workers and unrelated children. The same
invocation also records `peak_mold_worker_rss_bytes` (largest observed single
`mold`/`ld.mold` worker) and `peak_process_tree_rss_bytes` (largest sampled sum
of the driver and its observed descendants). Sampling runs every 100 ms using
Linux `/proc/*/stat` or macOS `ps`; observed descendants stay attributed by PID
and start identity after reparenting. The worker peak includes its PID, start
identity and sample timestamp for comparison with a synchronized external
sampler. Tree sums may double-count shared pages, and short-lived workers that
fork and reparent between samples may be missed. These are sampled lower
bounds, not a complete whole-link high-water mark. `rss_sampling_status`,
errors, sample count and bounded post-driver drain report incomplete evidence;
no samples produce null values rather than a fabricated zero peak. A zero
worker peak with samples means no named mold worker was observed.
The added observations do not change native linker flags, process groups,
leases, admission estimates or the memory reserve guard. `estimate_exceeded`
still checks the waited driver; `sampled_tree_estimate_exceeded` is observational.
The external 2.1 GiB estimate above still sizes links pending same-link native
assembly calibration. The producer start budget reserves one link; additional
links require fresh pool admission.
Supervisor memory/PSI samples must validate the estimates on each host.
Insufficient concurrent capacity selects sequential legs with a fresh memory
admission before each phase. `CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS`
(default 600) bounds memory/slot waits and compile pauses;
`CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLL_SECS` (default 1 for guards, 2 for phase
admission) controls resampling. Both accept positive integers. Each refusal and
later admission is recorded, including on timeout. The compile guard pauses the
producer process group within 2 GiB of the reserve and resumes once 4 GiB is
available above it; a deadline or observed reserve breach aborts the producer
and prevents PASS. It records every sample and pause/resume in
`compile-memory.jsonl`; the assembly receipt includes both guard and link logs.
Native linker selection, Cargo target rustflags, explicit environment flags and
the worker job ceiling are preserved. Immutable helper paths keep clone-path
changes out of the shared Cargo dependency fingerprint. Configurations using
`cfg(...)` target rustflags must supply explicit native flags for this guard.
Missing memory probes fail admission. Consumers have a fresh
thread ceiling using an assumed 4 GiB base plus 256 MiB per test thread, and
never overlap a producer. These are admission estimates, not OS memory limits.

The gate prints a reuse hit for both suite rows or a `MISS assembly key=…`
reason. Environment misses from new receipts also name the first changed
variable; receipts store only per-variable hashes. `CAS_RELEASE_ARTIFACTS_ROOT`
and `CAS_RELEASE_RECEIPTS_RUN_DIR` are output locations and do not invalidate
proof. Compiler flags, HOME, PATH, local environment/config files and the
resolved Zig binary remain inputs.

The environment fingerprint includes every variable passed to the test rows by
default, including unknown `CAS_*` variables. Assembly first removes the exact
harness/session names in `scripts/assembly-proof.py`'s `IDENTITY` set from both
the test environment and the fingerprint: factory/agent/session identity,
`CAS_ROOT`, `CAS_CLONE_PATH`, `AI_AGENT`, `CLAUDECODE`,
`CLAUDE_CODE_CHILD_SESSION`, `CAS_FACTORY_MODE`, `CAS_FACTORY_SUPERVISOR_CLI`
and `CAS_FACTORY_WORKER_CLI`. A factory shell and a scrubbed release shell can
therefore share the same proof without passing harness context to test children.
There is no blanket `CAS_FACTORY_*` exclusion: build controls such as
`CAS_FACTORY_CARGO_BUILD_JOBS`, test safety controls such as
`CAS_TEST_PROTECTED_DBS`, and compiler flags such as `RUSTFLAGS` remain inputs.
The explicit `VOLATILE` set excludes shell bookkeeping, build/output locations
and `CAS_RELEASE_ENV_FILE` (the publisher's env-file locator); values loaded
from that file still count under their own names. Release gate/train
orchestration variables are also excluded. Zig is keyed by binary contents
instead of its worktree path; ignored local environment/config files are keyed
by contents too. New exclusions require confirming that they cannot change the
compiled candidate or test behavior.

Rolling integration runs the release gate's no-build rows before builder
admission, plus `ci-script-tests` with every declared release-train control
exported. `integration.json` records each named row as PASS or FAIL under
`no_build`, bound to the integration tip. Train preflight refuses a missing,
stale or failed row before assembly and names the blocker; rerun
`cas factory integration-recover` after fixing the tip. Generic projects
without Cassy's release gate keep their existing runner.

All gate suite children use `scripts/release-test-env.sh`: it removes harness
identity, train/gate control namespaces and receipt destinations, provides a
clean temporary HOME and disables global Git configuration. Cargo and Rustup
locations remain explicit. The parent retains its orchestration environment;
validated host memory admission and compiler linker resource context survive
so nested suites and linkers remain admitted without inheriting release knobs.

For daemon-initiated sweeps, persist the scratch base with
`cas config set factory.release_gate_home_dir /home/cas-release-gate/base`
in the project's `.cas/config.toml`. The daemon passes this key to assembly
as `CAS_RELEASE_GATE_HOME_DIR`, overriding inherited shell and sweep env values.
An unset or blank key reports `NOT CONFIGURED`, names the key, and skips
the suite and failure attribution. Other projects and configured sweep commands
retain their detected or configured runner without requiring this key.

For manual assembly commands, set `CAS_RELEASE_GATE_HOME_DIR` to a scratch
base on the checkout filesystem
outside `/tmp`, `/var/tmp`, `/private/tmp`, `/private/var/tmp`, the configured
`TMPDIR`, and every `.cas` ancestor (for example,
`CAS_RELEASE_GATE_HOME_DIR=/home/cas-release-gate/base` on Linux or
`/Users/Shared/cas-release-gate/base` on macOS). The plain clone must be outside
Cassy's disposable roots so discovery and update tests exercise durable
projects. Assembly refuses an unsafe base before tool probing or either suite;
its legacy `/var/tmp` default requires this explicit override.

The script tier must pass, and both Rust contexts must report nonzero passed
tests, before an atomic PASS is written
under the shared `.cas/merge-sweeps/assembly-proofs/` directory. The receipt
records the tested Git tree, script-tier status/tree/log, each Rust context's
tree and pass count, per-leg and compile-phase intervals and CPU timings,
memory scheduling decisions and serial fallback reasons, toolchain,
environment and archive size. Full Cassy integration sweeps and the train's
assembly stage use this same command; retries cite the existing receipt.

Before the pipeline lands, `--cut --resume` compares the integration tip/base
with the input recorded by assemble. A changed integration input archives the
old stage receipts and reruns assemble, prep, ledger and every later stage.
Release prose, journey-evaluation reports, member-version bumps and the
generated ledger are replayed onto the new tested tip; source edits block
automatic replay. A rebase conflict
restores the checkout and prints a named blocker with a recovery command.
After a valid pipeline/publish receipt exists, resume finishes that landed
release without adopting a newer integration tip.

The first full release gate automatically reuses its nextest and archive-mode
rows from a matching receipt. `--only` remains a fresh diagnostic. Receipts
expire after 24 hours; dirty checkouts, changed code/manifests/scripts/workflows,
toolchain or test environment cause a miss. `CHANGELOG.md`, release prose under
`docs/release-notes/` and `docs/release-reports/`, and Markdown under
`docs/qa/journey-evaluations/` are excluded from the code-input hash; embedded
Rust documentation fixtures remain inputs.
The prep stage's workspace-member `[package]` version values and corresponding
source-less member `[[package]]` lock versions are normalized. The generated
`cas-cli/src/builtins/reference-history.json` ledger is excluded; its source
references and generator remain inputs. Every other manifest or lock byte,
including dependency and non-member versions, still requires a new proof.
The helper uses Python 3.11's standard-library TOML parser.

Gate evidence: PR #655/run 33430464567; PR #657/run 33435093275.

### Release delivery completion

After user-facing changes reach `main`, a source merge is not the delivery
completion receipt. The existing release train's `--host-update` stage also
runs `scripts/release-completion.py`; `--cut` requires its PASS before finishing,
including when an external host-update stage is used or a previous stage-done
marker exists. No additional CI lane is involved.

`delivery-completion.json` binds the bumped runtime manifest/version, previous
release tag, complete commit interval through the landed SHA, refreshed
`origin/main`, exact successful publication workflow, and published asset
digests. The gate downloads the host's release archive, installs its binary into
an empty temporary home, checks its version and clean build commit, and compares
its bytes with the updated host binary. Existing host, hub and refresh convergence
proof is also required. A later merge on `main`, a stale same-version build,
missing publication, or a deferred update fails completion. The main ref is
checked again after installation. Failure replaces any earlier PASS receipt.

Set `CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO` to the operator's explicit reason to
hold announcements. The cut records it in `announcement-embargo.txt`, keeps
announce/report/receipts pending, and continues runtime publication and install
proof. Standalone announce/report also honor it. Omission on resume preserves
the embargo; explicitly setting it to an empty string lifts it. Resume then
finishes the pending announcement evidence without republishing the runtime.
An embargo never waives the publication or install requirement.

### Supervisor proof targets

Run `python3 scripts/assembly-proof.py prove <epic-worktree>` for assembly.
Native and archive-clone producers each use their own `<worktree>/target`;
`CARGO_TARGET_DIR` inherited from a different lane is replaced. For a scoped
check, use `python3 scripts/proof_target.py run <worktree> -- cargo check --workspace --tests`.
The scoped test wrapper applies the same target isolation. Command-line
`--target-dir` overrides are refused, and every proof log records the source
worktree, HEAD and target path. A target whose recorded source root differs is
refused, including symlinked targets.

The immutable `.cas/build-cache/current` snapshot seeds compiled dependencies
with hardlinks where supported. Cargo freshness metadata is copied privately;
workspace fingerprints/artifacts, incremental state and Cargo locks are never
seeded. Workspace freshness is also discarded when adopting a legacy target or
changing HEAD, so old source mtimes cannot bless another tree's exports.

### Worker build caches

Factory worker spawns use `sccache` automatically when it is installed, while
keeping a separate target directory per worktree so concurrent Cargo builds do
not serialize. An existing `RUSTC_WRAPPER` wins; set
`CAS_FACTORY_DISABLE_SCCACHE=1` for the emergency opt-out. CI uses the GitHub
cache-v2 backend and keeps the cold Build Benchmark explicitly uncached.

Capped worker checks and named tests override both `RUSTC_WRAPPER` and
`RUSTC_WORKSPACE_WRAPPER` with empty strings, including Cargo-config and
`CARGO_BUILD_*` fallbacks. Their Cargo/rustc descendants inherit a private target
lifetime lease; a compiler-cache daemon must not retain it after Cargo exits.
Prestarting sccache cannot prevent its client from spawning another daemon if
the server exits. Supervisor and CI builds keep their configured wrappers.

When a worker delivery parks awaiting merge or closes, Cassy keeps only
`factory.target_cache_retention_count` warm parked check caches (default: 1).
It prunes the other private `target/debug` outputs under the same per-worktree
lane lock used by the capped runner, with a Cargo-lock and open-output check.
Place durable check logs at `target/worker-check.log`, nextest reports under
`target/nextest`, or in the task artifact directory; these survive pruning.
The next check re-seeds missing debug outputs from the immutable baseline.
Active builders and open test/output handles prevent reclamation. Source files,
receipts and the baseline stay intact; concurrent workers keep independent targets.

Actual worker shutdown also reclaims the complete `target/` after lane, Cargo
lock, registered-checkout and process checks. Before deletion, non-build files
including check logs and `nextest` receipts are copied, synced and verified in
`<project-artifacts>/<last-task>/retired-target/<worker>/<head>-<attempt>/`.
A worker without an associated task uses the inventory-only `_retired-workers`
namespace. Recycle keeps its warm target; parked checkouts remain registered.
Failed evidence copying preserves the target and reports the deferred path and
retained bytes. Newly CAS-created private targets have a durable external
record in `.cas/worker-target-owners`, binding checkout and target device/inode,
plus a unique generation marker inside the target to defeat inode reuse,
lease inode, creator/builder PID start time and Linux boot identity. This record
precedes seeding or build data. The capped runner holds the target lifetime
lease. Checkout directories may use ordinary group-writable Git permissions;
the target and ownership directory are created privately, and the marker,
record and lease files reject group or world writes. The runner passes the
lease to descendants; lane and slot locks remain private to the
runner. A held lease or matching live owner prevents retirement, including after
the runner dies. Ownership markers require `target/` to be ignored already.
Otherwise acquisition places no marker, reports legacy output in the trace and
continues with retirement disabled. CAS never edits the operator's Git
configuration or shared exclusions to make a checkout clean.
Retirement holds the same lease through evidence copying and
quarantine, revalidating ownership before deletion. On Linux only, verified
lease-managed targets tolerate opaque unrelated processes while still checking
every readable output handle, executable and mapping. Unknown legacy targets,
replaced inodes and unavailable owner identities remain retained with bytes;
they are never silently adopted. macOS keeps the conservative `lsof` probe.

Assembly and release scratch uses an owner record, PID start-time identity and
an inherited lifetime flock. TERM, INT and HUP stop and reap child groups before
cleanup; the parent allows 20 seconds for a nested guard's 5-second escalation.
A dead lease-managed owner may be swept despite opaque unrelated processes.
Unknown-provenance paths remain fail-closed. Fresh archive remaps record a
`cas-release-remap-v1` receipt binding the scratch owner's PID/start identity
and base inode to the detached HEAD, checkout/Git-pointer inodes and exact Git
common/admin/registry directory identities. After the owner dies and its
inherited lifetime lease is exclusively acquired, cleanup revalidates this
receipt and removes only the clean, unlocked generated remap with
`git worktree remove` (without force). The same targeted operation removes a
receipted missing checkout's stale admin entry; cleanup never runs a global
`git worktree prune` that could discard missing parked deliveries.
Normal guard teardown uses the same checks after children are reaped and the
inherited lease is released; live output handles preserve the remap first.
Malformed receipts, symlinked Git metadata, changed identities, branches,
dirty checkouts and Cassy worker/parked provenance preserve the base. A report
validates identities without changing the checkout or registry. Whole-base
reclamation follows only after the exact remap is unregistered and remaining
registrations are rechecked. Other registered remaps and their bases stay
intact; only dead owned bases' `suite.tar.zst`, `extract`, `tmp`, `cargo-home`
and `bin` siblings may be reclaimed. The operator's main checkout stays
registered and intact.
`gc_report` includes paths, reclaimed/reclaimable and retained bytes in
`RELEASE_SCRATCH_STATUS_JSON`; scratch `gc_cleanup` requires both `force=true`
and `dry_run=false`. Reports never create locks or owner records.

Verified dead owners are swept on the next start, including fresh SIGKILL
leftovers; age only protects unknown provenance. A surviving descendant's
inherited flock defers cleanup even after its parent exits. The guardian owns
Bash temporary directories too, so an EXIT trap cannot outrun child teardown.
`CAS_RELEASE_SCRATCH_MAX_AGE_HOURS` defaults to 6 for unknown paths.
Each assembly clone uses its own `repo/target` inside owned scratch. A
`BoundedCache` lease encloses that target's actual use and records cap/age
cleanup before and after the proof: `CAS_ASSEMBLY_TARGET_MAX_GIB` defaults to
20 GiB and `CAS_ASSEMBLY_TARGET_MAX_AGE_DAYS` to 7 days. Whole-clone teardown
still waits for inherited child leases. Only immutable worker dependency
snapshots seed the clone; workspace artifacts and freshness are private.
Receipts and `gc_report` retain both legacy `assembly-target` and
`assembly-target-leased-v1` inventory paths. Neither is a Cargo proof target.
Unknown, live or opaque caches are retained without silent adoption. Explicit adoption is
available only in a quiet window and refuses held leases or unknown/live users:

```bash
python3 scripts/release_scratch.py --repo "$PWD" \
  --cache .cas/merge-sweeps/assembly-target --adopt-legacy-cache clean
```

On soundwave, opaque `systemd --user` evidence makes adoption refuse. The
supervisor must remove the reported legacy cache by hand in a quiet window,
after confirming no Cargo process is running and no cache lease is held. It was
29 GB at discovery; use the inventory receipt for its current byte count.
Never silently adopt or delete an unknown cache to bypass the liveness check.

Lane compile previews carry provenance and a lifetime owner lock.
Previews are direct children of `.cas/worktrees` so the existing private target
ownership check admits them. Their sibling metadata directory binds the exact
checkout path, Git common directory and commit and holds the lifetime lock.
GC also recognizes older nested `lane-compile-*/preview` checkouts.
Explicit `gc_cleanup force=true dry_run=false` removes stale owned detached previews,
including their Git registration, after revalidating ownership, process liveness
and `factory.target_cache_min_idle_secs`. Recent or live previews survive;
previews created before provenance was recorded remain inventory-only. The
`TARGET_CACHE_STATUS_JSON` report includes preview target sizes and
`lane_previews` dispositions. The existing high/low watermark configuration
controls cache pressure warnings; preview cleanup does not need disk pressure.
On macOS, liveness uses NUL-delimited `lsof` field output and fails closed if
that probe is unavailable or reports errors. Only the evictor's held Cargo-lock
file descriptor is exempt; other same-process handles and executable/mapped
artifacts preserve the cache. Linux also inspects `/proc/PID/exe` and `maps`,
and unknown/inaccessible evidence keeps the cache; reclamation therefore needs
a readable process table. Same-HEAD parks have distinct marker generations,
so an older retention inventory cannot evict a newly parked warm cache.
Interrupted `.cas-parked-debug-*` quarantines remain preserved for explicit
whole-target GC once the worktree is inactive and the recency/pressure policy
permits it. They are included in target byte inventory; automatic park-time
cleanup does not retry a quarantine or delete proof logs to recover it.

New isolated workers also seed their private `target/` from compiled artifacts
hardlinked out of the quiescent snapshot named by `.cas/build-cache/current`;
small Cargo dep-info files are copied with their target root rebased. The release
train's host-update stage refreshes that baseline automatically from a detached
checkout of the released tag on main. `host-update.json` records the completed
snapshot ID and source commit; a refresh failure warns without failing the
published release. Run `scripts/refresh-worker-build-cache.sh` for a manual refresh
after an epic integration merge. The script builds a new snapshot to
completion and only then publishes its pointer, so no worker ever seeds from a
live Cargo writer. Old snapshots remain valid for in-flight seeders and should
only be removed during a maintenance window. Set
`CAS_FACTORY_DISABLE_TARGET_SEED=1` to skip seeding. Do not replace this with a
shared live `CARGO_TARGET_DIR`: its Cargo lock serializes the worker fleet.

Worker provisioning (including store and Git base resolution) runs in a separate
process group with a five-minute deadline. Timeout, targeted shutdown and
`restart_spawn_queue` kill only that generation's provisioner and descendants;
the daemon keeps processing shutdowns and messages. Reset drops already-dequeued
spawn actions and reports them; persistent queue rows continue draining. Retirement cleans newly-created checkout/branch metadata and worker-specific
Git locks best-effort; reused worktrees are preserved. Inspect any reported
leftovers before retrying.

Before creating or reusing a checkout, `factory.spawn_min_free_gib` checks
available space on the worker filesystem (default 25 GiB; 0 disables the floor).
Below the floor, the spawn fails with `spawn_disk_floor` before a checkout or
branch is created. Set it with `cas config set factory.spawn_min_free_gib 30`.
The target repository's config applies to cross-repository workers. Existing
usable targets and `CAS_FACTORY_DISABLE_TARGET_SEED=1` bypass seeding, while
all spawn paths still apply the disk floor. Seeding normally hardlinks artifacts;
the floor reserves space for subsequent worker writes and builds.

Local sccache 0.10.0 does not produce cross-worktree Rust hits because absolute
checkout paths remain in its cache keys (measured 0/45 hits even with
`--remap-path-prefix`). Supervisor and CI builds keep sccache enabled for when
[upstream path normalization](https://github.com/mozilla/sccache/pull/2678)
lands; hardlink seeding is the current cross-worktree mechanism. Capped worker
builds reuse their private Cargo target without a compiler-cache daemon.

### CI-load policy

Standing operator policy: factory/* pushes and epic-targeted PRs run
import/dependency-selected tests in Scoped Validation. Protected-default PRs run
only the required Fast Validation and macOS Check admission lanes. The release
merge queue runs the complete workspace suite on its synthetic tree once; when its
successful tree is pushed unchanged to main, the main-push Fast Validation and
macOS lanes reuse that receipt and name the validating run. Direct pushes,
bypass merges, receipt lookup failures, and changed trees still run those
lanes. The non-required full/heavy tier (Clippy, Test Compile Guard, Build
Benchmark, and both Panic Isolation profiles) belongs only to
supervisor-controlled main pushes, schedules, or manual dispatches—never
factory/*, epic/*, tags, or pull requests. Keep this policy pinned by
`scripts/test-ci-test-tiers.sh`, rather than relying on convention.
[Change-scoped CI](../../docs/ci/test-impact.md) describes selection, additive
failure history, count/time receipts and full-suite recall measurements. Docs-only
diffs (paths under `docs/` or Markdown files outside embedded
`cas-cli/src/` content) on pull-request, push, and merge-group events route
only to the `Docs Lint` job; it runs Markdown lint, validates any changed
release-note drafts, and checks that generated AGENTS.md files are current.
The existing required Fast Validation and macOS Check
contexts remain present and skip their full work for that class. Mixed and
code diffs keep the full required tier.

### Build profiles and the binary

The MCP server is always included because factory agents depend on `cas serve`; the optional `mcp-proxy` feature is enabled by default. Binary is `cas` (lib + bin in `cas-cli/`). Build script embeds git hash and build date.

Build profiles must use `panic = "unwind"`. The MCP tool-dispatch panic catcher relies on `tokio::spawn` + `JoinError::is_panic`, which only observes a panic if the worker thread unwinds. A compile-time guard in `cas-cli/src/lib.rs` refuses non-test builds with `panic = "abort"`; do not work around it, because the catcher is what keeps `cas serve` alive across handler bugs.

## Skill & Rule Sync

Cassy auto-syncs rules to `.claude/rules/` and skills to `.claude/skills/` as SKILL.md files with YAML frontmatter. The sync logic lives in `cas-cli/src/sync/`. Rule promotion uses configurable outcome evidence: `sync.promotion_threshold` defaults to 2 and `sync.promotion_evidence` accepts `helpful` and/or `retrieval`; one `mcp__cas__rule action=helpful` call never promotes. A reviewer's explicit decision uses `mcp__cas__rule action=promote id=<id> change_note="<why>"` instead of voting. Judgement-call decisions require two distinct `source_ids`; `rule action=update source_ids="<all contributing IDs>"` preserves merged evidence. An observed mechanical constraint tagged `enforceable:lint`, `enforceable:test`, `enforceable:hook`, `enforceable:gate`, or `enforceable:type` files one idempotent encode chore on creation or tagging, even while draft, and can be promoted after its first verified occurrence. Retired or harmful rules do not file chores. `factory action=epic_status` includes pending encode chores from the project backlog. Retrieval promotion requires useful outcomes across at least two distinct privacy-preserving sessions. Harmful feedback and negative retrieval outcomes require `sync.demotion_threshold` (default 2) before demoting Proven rules to Stale and removing their synced files. Existing Proven rules are grandfathered until new evidence crosses the configured threshold.

Built-in skills ship to every project, so they carry no cas-src-only procedure. This repository's own factory guidance (release prebuild, worker build-cache refresh, the assembly gate command, cargo triage) lives in [docs/factory/cas-src-factory-notes.md](../../docs/factory/cas-src-factory-notes.md) and [docs/factory/cas-src-worker-notes.md](../../docs/factory/cas-src-worker-notes.md).

### Skill validation contract

`validation_script` is an opt-in create/update gate. When present, Cassy runs it
before writing the skill to SQLite or syncing its `SKILL.md`; a zero exit status
admits the change and a non-zero status (or timeout) rejects it without creating
a version row. The probe runs through the platform shell from a fresh temporary
directory, with a scrubbed environment that retains only `PATH` for executable
lookup. On Linux, bubblewrap is used when available to provide a network
namespace with no routes. On hosts without bubblewrap, the default is a
degraded plain-shell sandbox with the same temporary cwd and scrubbed
environment; Cassy reports an explicit warning that network isolation is
unavailable. Set `skill_validation.require_sandbox = true` to fail closed
instead. The five-second timeout and process-group cleanup bound the MCP
request. Scripts are local, deterministic availability checks: they must not
depend on network access, inherited CAS credentials, project files, or
persistent relative writes. There is currently no network opt-in declaration,
so all validation scripts use the no-network policy. Validation output is
included in rejection errors and capped to keep responses bounded.

Skill `preconditions` and `postconditions` are advisory metadata: Cassy does
not execute or evaluate them. They are surfaced in `cas skill show` and in
generated `SKILL.md` sections so consumers can evaluate them in their own
runtime.

### Editing supervisor or worker guidance: the SessionStart budget is a hard constraint

`cas-supervisor.md` and `cas-worker.md` are not ordinary docs. Their bodies are
injected verbatim into the SessionStart `additionalContext`, which has a **9 216 B
aggregate budget** (`hooks::handlers::session_budget`). Role guidance is
*protected*: it is never compacted. Everything else — Ready Tasks, Helpful
Memories, Available Skills, the GitHub issue triage and its open-issue titles —
is degradable, so when the guidance grows, one of those is silently replaced by a
one-line "run this command instead" summary. Nobody sees an error; a section just
stops being there.

That is not hypothetical. In cas-caaf a 796 B role note pushed the payload over
budget and a supervisor lost the open-issue titles in live sessions, while CI
stayed green because the note only appears when `CAS_FACTORY_WORKER_CLI=codex`.

Three tests hold the line; if you grow the body, expect to meet them:

| Test | What it bounds |
| --- | --- |
| `test_supervisor_guidance_under_8kb` | the guidance as a component (8 000 B soft cap, 8 192 B ceiling) |
| `supervisor_guidance_leaves_room_for_the_rest_of_the_session_start_payload` | what the guidance *leaves* — at least `SESSION_START_GUIDANCE_REMAINDER_FLOOR_BYTES` (2 400 B) of the budget for every other section |
| `the_codex_worker_matrix_still_fits_the_session_start_budget` | the same bound with the Codex worker note appended (`CAS_FACTORY_WORKER_CLI` unset and `codex`) |

The remedy when one fails is always the same: move detail into
`cas-supervisor/references/` (or `cas-worker/references/`) and leave a pointer.
The body is for rules that apply every session; everything else is one named
command away.

### Builtin skill references

Every file in a managed builtin skill directory other than its `SKILL.md` (references, scripts, examples, templates) is owned by that skill and synced with a baseline ledger: a destination that differs from both the recorded baseline and every version Cassy has shipped is preserved as a local customization (and surfaced in a SessionStart banner). The set of "versions Cassy has shipped" is the embedded `cas-cli/src/builtins/reference-history.json`. The ledger keeps deleted files too: sync prunes an installed file Cassy no longer ships only when its content proves it is Cassy's (a shipped hash, the recorded baseline, or `managed_by: cas`). Retired managed agents are pruned the same way, and `cas doctor` reports installed-vs-catalog drift as `host install parity`.

**After changing, adding or deleting any builtin skill file — and before cutting a release — run:**

```bash
./scripts/gen-builtin-reference-history.sh
```

and commit the regenerated JSON. Skipping it means the version you just replaced is not recognized as Cassy content downstream, so installs that still hold it will keep it forever instead of upgrading (cas-0c0a).

### Viktor distribution

The managed Viktor surface has three coupled user-facing pieces: its Claude/Codex/Grok builtin
skill mirrors, `cas viktor` credential-safe provisioning output, and the proxy's configured
allowlist. Keep the skill body compact and put operational detail in its reference. The mirrors
must retain identical meaning after their mechanical tool-prefix substitutions; run the builtin
flavor-drift test. Never add a credential literal to source, fixtures, docs, or artifacts. The
only supported user entry is `cas viktor key`; it prompts, validates, then saves the
key in machine-scoped state, while `cas serve` still holds only the `VIKTOR_API_KEY` reference.
For a new CLI or proxy-facing behavior, add a clean-project `cas init`/command assertion and a
direct registry test so the source cannot exist without reaching all three downstream harnesses.

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

### Task close delivery attribution

Close posture checks (`additive-only`, `value-only`), no-code intent, and the
receipt diff stat share task delivery attribution in
`mcp/tools/core/task/lifecycle/close_ops/task_attribution.rs`. The remote-tracking
integration target is preferred when present. Unmerged commits are bounded by
the task work window; already integrated commits need task identity. An explicit
commit receipt caps the displayed history and includes unnamed predecessor
commits within the work window, stopping at another task's commit. Receipt
inputs remain hexadecimal commit IDs, including unambiguous abbreviations.

The close gate resolves the recorded factory branch locally or on origin before
counting commits. An unavailable branch produces a missing-evidence error and
is not parked for merge. A no-code task whose stale code target and delivery
anchor were cleared closes on a portable `external_ref`; retained code anchors
and commit receipts still require delivery proof.

A live registered supervisor may repair an incorrect execution methodology with
`task action=update id=<task> proof_scope_fix=true execution_note="" reason="<why>"`
to clear it, or supply a valid replacement methodology. Switching to `no-code`
requires a portable `external_ref`, either stored or supplied in the same update.
This correction invalidates the old verification cycle and reopens the task with
its assignee, delivery anchors and immutable merge facts preserved. It supports
task-only investigation proofs as well as merged code deliveries. Ordinary close
proofs still apply: declaring `no-code` never hides delivered code. Correct only
one of methodology, work target, proof targets or risk in each update.

A passed or waived independent QA round remains bound to its reviewed tip after
a squash merge. Close proves that the integrated receipt carries the same trees
over the aggregate delivered paths, or the same stable aggregate patch ID.
Unrelated target files do not change coverage. An unresolved or changed delivery
still refuses, with review coverage and integration proof reported separately.

A live registered supervisor may use `supervisor_override=true` with a non-empty
reason to waive additive-only/value-only posture checks and the receipt epoch
check for a retroactive record task. Close records the decision. Repository
binding, ancestry, non-empty delivery, and target-content checks still apply.

A delivery's final file that is byte-identical on the authoritative target and
still differs from the task's delivery base is present, even when historical
intermediate builds disappeared during recovery. This exact-file proof applies
to any path; it grants no exemption for minified or generated files. Child close
and epic accounting use task-attributed history for merge deliveries rather
than crediting only the epic changes imported by a worker's sync merge. Epic
accounting compares the recorded child anchor, so a later task on the same lane
cannot supply its final snapshot. Exact-file recovery also requires a
task-attributed path effect to survive, including attributed side-parent work. Restoring any imported path baseline, even
one newer than the task's original base, rejects both explicit and unlabeled
inverse changes unless a supervisor records the audited supersession below.

For an older runtime that falsely reports dropped historical bundle lines,
first inspect the recorded anchor and authoritative target with `git ls-tree`
and `git diff <anchor>..<target> -- <paths>`. Preserve branches and evidence.
Upgrade to the runtime containing the exact-file proof, or use the authenticated
supervisor review below with the real integration/replacement commit and exact
blob comparison in the review. Do not manufacture a corrective commit or edit
task metadata directly. The cas-c2cb incident (cas-5f0b) used the tracked merge
64d1740bd7bb065049dea3b9c7eee2e712738e68 as its reviewed-drop receipt; anchor
6ff3c4d6e and target held app.js blob 5bde1a1d1bf2b077538c054ee758bc8ffcdd4006.

A deliberately superseded delivery that still fails automatic content attribution
can close with `supervisor_override=true` and
`reason="reviewed-drop: <superseding SHA>[,<SHA>...] -- <why>"`. Each named commit
must differ from the delivery anchor, be reachable on the authoritative target,
and change a dropped path in its first-parent diff. Together they must cover
every dropped path's final target state: a path counts only when its state at
the named commit matches the target, including deletion and file mode. The
superseding commits may predate the anchor, as when migration renames or a
deliberate revert landed before the delivery was re-anchored (GH #1160).
Cassy records the full resolved commit IDs, anchor, measured target, paths and
review. A narrative without commit receipts cannot waive the content gate.

When the content proof cannot decide (`DELIVERY CONTENT UNVERIFIABLE`, for
example a fix committed inside a merge resolution), a live registered supervisor
who has inspected the delivery can close with `supervisor_override=true` and
`reason="reviewed-content: <delivery SHA>[,<SHA>...] -- <what was inspected>"`.
Each named commit must be the delivery anchor or a descendant of it, be
reachable on the target, and carry a non-empty first-parent diff. Cassy records
the resolved commits, anchor, target and review as a decision note.

The epic close gate bounds ref/cache metadata at 8 s, then uses an 8 s soft
proof budget. The first uncached child's delivery/history proof may finish
beyond that soft budget under a separate 20 s hard cap, guaranteeing progress
when a loaded host takes longer than a call's budget to prove one child.
Missing-anchor fetches and subsequent proofs retain the soft budget; summary
views and zero-budget cache-only calls never receive this allowance. Bounded
close/status collection takes at most 8 + max(soft budget, 20) s (28 s for
the production budgets), leaving room below the 55 s MCP deadline. A probe that
exhausts its hard deadline is terminated with its descendants, and its unfinished
child remains unchecked rather than becoming a measured verdict. When it stops
early (`EPIC CLOSE CHECK INCOMPLETE`), the verdicts it already proved for
closed children are saved under the repository's common Git directory
(`cas/epic-close-verdicts.json`), keyed by the exact refs and anchor each proof
read. Retrying the same close reuses them and continues from the first
unchecked child, so a large epic closes after a few retries. A moved target,
lane or anchor invalidates only the affected verdicts. A measured stranded
child still rejects immediately. When the check is incomplete, no measured child
blocks and the task store shows every child terminal, a live registered
supervisor may instead close with `supervisor_override=true` and a reason; the
waived output, including every unchecked child, is recorded as a decision note.

An empty `execution_note` update may clear a constraint after approval when its
exact repository proof is unchanged. Pending, skipped, unbound, and changed
proofs remain locked; changing other scope fields or replacing the constraint
still requires a fresh proof cycle.

### Test shape and runner evidence

`python3 scripts/check-test-shape.py` checks tracked Rust tests and test-only
helpers for constant/literal equality and reads of Rust source as text.
`--changed-since <ref>` checks the merge-base diff, including working changes;
the release fast rows and Scoped Validation run this form. Intentional external
wire or structural contracts carry `// pin: <reason>` immediately above the
statement or test/helper declaration, or on the assertion line. A reason does
not convert a source-order assertion into behavior coverage.

Workers and independent QA run `npm test`, `npm run typecheck` in `hub-web`,
and `scripts/journey-eval.sh <task-artifact-dir>` for source-impact-selected
journeys at four workers. The wrapper resolves the task's declared target;
`--affected <base>` binds an explicit base. No browser runs for an empty impact
selection; it emits an explicit receipt. Caller spec/grep filters are refused.

Factory PreToolUse denies worker/reviewer `--full`, unfiltered Playwright and
unfiltered journey npm scripts. Named spec or canonical-ID runs remain allowed
for iteration; a failing spec may be rerun at one worker, retaining the original
failure. Reuse implementer exact-tip receipts in independent QA and do not
rerun browsers for doc/ledger-only commits with unchanged evaluated inputs.
The supervisor runs `scripts/journey-eval.sh <epic-artifact-dir> --full --workers=4`
once at epic assembly; merge queue runs the full suite again. Receipts fold all
native parts by actual catalog ID and record full base/head, pass/fail/skip
counts, Playwright version and the native exit code. Native runners refuse
successful zero-test summaries and export the passing count to `VERIFIED_TEST_COUNT_FILE`
when requested. Rust re-exec helpers require the exact child name, one selected
test, and one passing result; intentional signal/atexit children instead prove
entry into the test body before their early exit.

The full release gate's `hub-web-tests` row runs `npm ci`, `npm run typecheck`,
and `npm test` before build and Rust suite rows. A failure prevents pipeline
admission. Its cached PASS depends on `hub-web`, `scripts`, and `.github`,
including the repo-level `scripts/visual-qa.mjs` that web tests import and read.
Both scoped CI lanes use the same conservative paths for `web-check-needed`;
Markdown fixtures beneath those paths still require web tests. The no-build
fast gate keeps its cheap rows, while the scoped lane runs the npm checks.

Closing a task that changes a committed `*.snap` or
`opencode_projection.snapshot.json` requires one task decision note per
changed snapshot file:
`snapshot-approved: <relative file> — file-sha256:<64 lowercase hex digits> — <why>`.
The digest covers the file's complete blob at the delivered tip, so one note
approves every changed line in it (a shared partial that alters 20 rendered
lines needs one note, not 20). Any later edit to the file changes the digest
and requires a new approval. The refusal prints the exact token for every
unapproved file. A snapshot deleted at the tip has no blob and uses the
line-level form, which also remains accepted for any file:
`snapshot-approved: <relative file> — <actual +added or -removed line> — <why>`.
The equivalent line-digest form is
`snapshot-approved: <relative file> — sha256:<64 lowercase hex digits> — <why>`.
SHA256 covers the complete UTF-8 diff line, including its leading `+`/`-` and
whitespace, excluding the newline. The refusal prints this bounded form when
the changed line exceeds 256 characters, so long prompts fit the default
1500-character note limit. Copy the suggested token and explain the reviewed
change; existing short-line literal approvals remain valid.
The close gate checks the task-attributed Git diff, even after merge, and names
the exact `task action=notes` command when approval is missing. Approval for a
different file or a line absent from that diff does not satisfy the gate.

## Operator write roots (GH #1169)

The factory workspace contract normally lets agents write only to their
worktree, `factory.artifacts_root/<project-key>/<task-id>/`, the configured
scratch root and the harness scratchpad. An operator can add directories
outside these:

- **Project write roots.** Run `cas config set factory.write_roots "~/soundwave-config/docs/requests,~/.config/autostart:create+edit+delete"`.
  Each entry is `path[:modes]`. The default mode is `create+edit`; `delete` is
  off unless named. An empty value clears the roots. `cas config get
  factory.write_roots` shows them.
- **One-off task grants.** Run `cas config grant-write --task <id> --path <dir>
  --mode create+edit --reason "<why>"`. The grant applies to the worker
  holding the task and to the supervisor until the task closes. It is
  recorded as a DECISION note on the task. `cas config revoke-write --task
  <id> [--path <dir>]` removes it.

Both are stored in `.cas/operator/write-policy.toml`, never in `config.toml`.
Paths are resolved to canonical absolute directories when set, so `..` and
symlinks are resolved rather than trusted. `/`, `$HOME` itself, and anything
that contains or lies inside `.cas/operator/` are refused.

Each root admits only its modes:

- **create**: a new file, an `apply_patch` add or move, or a `cp`, `tee` or
  `touch` destination;
- **edit**: an existing file, or, once a policy file exists, `sed -i` /
  `perl -i` operands and `mv` sources (without a policy file these keep their
  previous, unjudged behaviour);
- **delete**: `rm` or an `apply_patch` delete.

Every write admitted this way is logged as a `workspace_write_root_used`
event (tool, path, mode, root, task, agent) in the factory session log. A
refusal lists the roots in effect.

**Operator-only, guardrail-grade (not security-grade).** The operator and the
agents share a Unix user. The PreToolUse hook is the actual gate against
agents; the CLI checks are defence in depth. With no policy file, the default
contract is unchanged. The layers are:

- the commands refuse to run when they detect any agent environment variable
  (`CAS_AGENT_*`, `CAS_SESSION_ID`, `CAS_FACTORY_*`, `CAS_CLONE_PATH`,
  `CLAUDECODE`, `CLAUDE_CODE_*`, `CODEX_SANDBOX*`, `CODEX_THREAD_ID`);
- they refuse when an agent or Cassy server (claude, codex, `cas serve`,
  `cas factory`) is among the process's ancestors, or when the process runs
  inside a factory worker or server cgroup;
- they refuse without an interactive terminal, and need a typed confirmation;
- `Config::set` refuses the key, so the config TUI, import and every agent path
  that reaches it cannot set roots;
- PreToolUse refuses any agent Bash call of these commands, even when wrapped
  by `env`, `sudo`, `setsid`, `sh -c` or `script -c`, and any agent write,
  edit, delete or rename under `.cas/operator/`. This applies to workers and
  the supervisor alike.

A process that deliberately hides a same-user write from these checks is out
of reach. A hard boundary needs agents running as a separate Unix user.

## Durable task artifacts

`factory.artifacts_root` is the shared parent (default `~/.cas/artifacts`).
New task evidence lives in `<base>/<project-key>/<task-id>/`; assignment briefs
print the exact path. The key combines the project folder label with a SHA256
of the canonical shared Cassy store path, so equal folder names and task IDs
in different stores stay separate, while symlink aliases and factory workers
using that shared store agree. Keep evidence paths in task notes or published
artifact records when a project moves.

Existing `<base>/<task-id>/` files remain readable and publishable. Completion
receipts and QA citations accept those historical paths. They are excluded
from automatic project cleanup: a flat directory can contain more than one
project's evidence. New writes, issue attachments, QA rounds, message spills,
search discovery and cleanup use the scoped namespace. No automatic file move
or ownership guess is made for legacy directories.

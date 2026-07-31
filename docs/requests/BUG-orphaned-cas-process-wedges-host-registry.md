# BUG: integration tests run `cas init --yes` against the developer's REAL `~/.cas` — lock contention, host-state pollution, and a 6-hour wedge

**Observed:** 2026-07-31, cas-src supervisor session `wise-viper-85`, cas 2.38.1 (3c0e189). Recurring — operator reports several occurrences; logs confirm 2026-07-29 and 2026-07-31.

> **Note on this document's history.** It was first filed as "an orphaned process should be reaped." The operator pointed out that *only a human should ever be running `cas init`* — which reframed the whole thing. The orphan was not stray operator activity; it was a **test child**, and the real defect is that the test suite writes to the host registry at all. Reaping is now the secondary concern. The corrected chain is below.

## Symptom

`task action=create` fails with:

```
MCP error -32602: WORK TARGET REJECTED: failed to register /home/pippenz/Petrastella/cas-src
in the host known-repo registry: database error: database is locked
```

The message reads as though the *repo argument* was rejected, so the natural response is to go debug the repo path or branch — neither of which is the problem.

## Root cause chain (verified)

### 1. `host_cas_dir()` follows the real HOME

`cas-cli/src/store/known_repos.rs:36-40`:

```rust
pub fn host_cas_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".cas")
}
```

`open_host_known_repo_store()` then `create_dir_all`s that path and opens `~/.cas/cas.db`. `cas init` calls `ensure_host_schema()` + `register_repo()` against it — by design, for a real user invocation.

### 2. Integration tests spawn `cas init --yes` without isolating HOME

`cas init --yes` is overwhelmingly a **test** invocation — the `--yes` non-interactive flag is an automation signature, and the wedged process was running the **debug** binary (`target/debug/cas`) from inside a worker's worktree, which is what `cargo test` uses.

The shared fixture `cas-cli/tests/fixtures/cas_instance.rs:246-262` is representative:

```rust
let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cas"));
cmd.current_dir(temp_dir.path());
cmd.env_remove("CAS_ROOT");   // clears CAS_ROOT ...
cmd.args(["init", "--yes"]);  // ... but never touches HOME
```

`CAS_ROOT` is cleared; **HOME is not**. So the spawned child resolves `host_cas_dir()` to the developer's real `~/.cas` and writes to the real `~/.cas/cas.db`.

12 test files spawn `cas init --yes`; only 8 set `HOME`. The 8 that do **not**:

```
cas-cli/tests/blame_attribution_test.rs
cas-cli/tests/component_output_test.rs
cas-cli/tests/fixtures/cas_instance.rs        <-- shared fixture, widest blast radius
cas-cli/tests/hooks_test/mod.rs
cas-cli/tests/jail_guard_test.rs
cas-cli/tests/loop_test.rs
cas-cli/tests/verification_test.rs
cas-cli/tests/verifier_handoff_cleanup_test.rs
```

Reproduce the list with:
`comm -23 <(grep -rl 'init", "--yes"' cas-cli/tests/ | sort) <(grep -rl 'env("HOME"' cas-cli/tests/ | sort)`

Unit tests already do this correctly — `known_repos.rs:163` uses `TestEnvGuard::run_with_temp_home`. The integration fixtures never adopted it.

Prior art: **cas-66a7** (P2, CLOSED) — *"Make spawned CAS integration sandboxes isolate host HOME and known-repo state."* That is precisely this bug. The fix did not cover these 8 files, or it regressed. Read that ticket before designing.

### 3. Parallel `cargo test` → many concurrent writers on one host DB

`.cas/logs/cas-2026-07-31.log` contains 24 `database is locked` entries. 22 are the *silent* variant, tightly clustered during test-run windows:

```
14:13:47  WARN cas::store::known_repos: failed to register repo in host known_repos registry (non-fatal)
          path=/home/pippenz/Petrastella/cas-src error=database error: database is locked
   ... 21 more, 14:13 through 19:41 ...
19:43:42  WARN rmcp::service: response error id=10 "WORK TARGET REJECTED: failed to register ..."
```

Six hours of invisible degradation, surfacing only when it reached the one call site that treats the failure as fatal.

### 4. One test child wedged and was orphaned

```
PID     PPID  STAT  ELAPSED   WCHAN        CMD
2439067 3204  SNl   06:22:09  get_signal   .cas/worktrees/bright-finch-83/target/debug/cas init --yes
```

Reparented to `systemd --user` (PPID 3204) after its worker died; wedged in `get_signal`; ignored `SIGTERM`, needed `SIGKILL`; held the host write lock 6h22m.

There is precedent for `cas init` hanging on stdin: `cas-cli/tests/cli_test.rs:84-88` documents a production hang where `select()` looped forever at 100% CPU on EOF'd stdin, fixed in `cas-cli/src/cli/interactive.rs`. This wedge is in the same family (stdin/signal handling) but is **not** the same failure — worth investigating rather than assuming it is covered.

## Why this matters beyond the lock

The lock is the visible symptom. The more serious issue is that **the test suite has been writing into the developer's real host registry** — registering temp-dir repos, running host DDL, mutating `known_repos` — on every `cargo test` run. That is state pollution of real operator data, and it makes host-registry behavior untestable and non-deterministic. It also means every worker running the full suite fights every other worker for the same lock.

## Do NOT "fix" this by raising the busy timeout

`SqliteKnownRepoStore::open` (`crates/cas-store/src/known_repo_store.rs:114`) already goes through `shared_db::shared_connection`, which sets a 5s `busy_timeout` (`crates/cas-store/src/shared_db.rs:46`). That is correct. No timeout value survives a six-hour hold, and raising it only makes contention slower and more confusing.

## Fixes, in priority order

1. **Isolate HOME for every spawned `cas` child in tests.** Point the 8 files above at a temp HOME (extend `TestEnvGuard::run_with_temp_home` to the integration fixtures, or set `HOME` in the shared fixture). This is the actual fix.
2. **Fail loudly if a test ever touches the host `~/.cas`.** A guard that panics when `host_cas_dir()` resolves under a real HOME during tests stops this from silently regressing a third time — cas-66a7 already fixed it once.
3. **Reap orphaned CAS child processes** on worker/session teardown. Prior art: **cas-82fb** (closed) reaped worker-spawned dev servers but not CAS's own binary.
4. **Separate the error classes** at `cas-cli/src/mcp/tools/core/task/repo_context.rs:339`, which maps any registry error to `WORK TARGET REJECTED`. "Registry momentarily unwritable" is not "your repo path is invalid." Compare `factory_preflight.rs:488-503`, which has proper remediation text; this path has none.
5. **Reconcile the fatal/non-fatal contract.** `known_repos.rs:128-145` documents `register_repo` as *"Non-fatal by design — losing the upsert must not break the primary operation"*, yet `repo_context.rs:339` breaks the primary operation. Pick one and make both honor it.
6. **Surface orphan processes pinning the host DB in `gc_report`**, which already reports stale agents and orphan worktrees.

## Operator workaround (until fixed)

```
fuser -v ~/.cas/cas.db                            # names the holding PID
ps -o pid,ppid,stat,etime,wchan:20,cmd -p <pid>   # confirm it is orphaned
```

If PPID is `systemd --user` and elapsed time is long, the owning worker is gone and the process is safe to kill. `SIGTERM` first; `SIGKILL` if wedged in `get_signal`.

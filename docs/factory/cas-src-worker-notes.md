# cas-src worker notes

Text removed from the shipped `cas-worker` references because it is specific to
the Cassy source repository or its factory host. Builtin skills ship to every
project, so these notes live here instead. Each section names the file it came
from. The text is verbatim except that headings are demoted one level and bare
code fences carry a `text` language.

## From `cas-worker/references/close-gate.md` — clean-tree receipt

**Do not assume a rejected tool call left the disk untouched.** On 2026-08-06 (cas-f102) an `Edit` returned REJECTED and the write landed on disk anyway. Nothing unreviewed shipped only because that worker ran `git status` unprompted, saw the divergence, and reverted. A tool result reports what the harness intended; `git status` reports what is true. When they disagree, believe git.

## From `cas-worker/references/close-gate.md` — Rust blast radius table

The shipped table now names library and shared crates generically. The cas-src
rows were:

| What you changed | What to trace by reading and `rg` |
| --- | --- |
| Internal logic, private functions only | Callers inside the crate |
| Public type in `crates/*/src/lib.rs` — new/removed field, changed signature | **Every consumer across the workspace** |
| Anything in `crates/cas-mux`, `crates/cas-factory`, `crates/cas-types` | **Every consumer in `cas-cli`** |

**Historical note (cas-c0e0):** two fields were added to `FactoryConfig` (cas-factory). Per-crate tests passed. `cas-cli` constructors failed E0063 at workspace scope. The regression shipped to main as commit `3dc7488` and was caught only during manual merge.

## From `cas-worker/references/discipline.md` — non-Rust suites and clean-CI wrapper

Non-Rust suites (for example hub-web `npm`/`vitest`/`playwright`) still run in

say so in the close note; the supervisor's assembly run then uses the project's
clean-environment wrapper:

```bash
make -C cas-cli test-clean-env
make -C cas-cli test-clean-env CLEAN_ENV_ARGS='--lib cloud::config'
```

The wrapper enumerates and strips the live `CAS_*` variables; do not hand-write
an `env -u` list. In particular, `CAS_ROOT` and `CAS_CLONE_PATH` can redirect a
test to the main checkout's `.cas`. There is no `CAS_TASK_ID`.

## From `cas-worker/references/recovery.md` — stuck builds and test binaries

Workers no longer run Rust builds, so this triage belongs to whoever builds at
epic assembly.

### A build that looks stuck: killed vs wedged

These are different failures with the same symptom (no output, no progress), and
telling them apart takes about ten seconds. **Do not wait it out** — a wedged
build never recovers on its own, and one was observed sitting for 57 minutes.

**First, read the build log, not the clock.** Cargo already reports a killed
child clearly:

```text
error: could not compile `foo` (signal: 9, SIGKILL: kill)
```

If you see that, the build **failed** — it did not hang. Something killed the
compiler. Re-run it. If it recurs, find out who is sending the signal before
blaming the machine.

**If there is no such line and nothing is moving, inspect the process:**

```bash
ps -eo pid,etime,time,stat,wchan:20,comm | grep -E "rustc|cargo"
```

Read two columns together:

| `TIME` (CPU used) | Meaning |
| --- | --- |
| climbing | It is compiling. Slow ≠ stuck. Leave it alone. |
| ~0:00 with large `ETIME` | Wedged. It has been alive for minutes and burned no CPU. |

Confirm before concluding it is a resource problem:

```bash
grep oom_kill /proc/vmstat        # 0 => the kernel has killed nothing, ever
cat /proc/pressure/memory         # "full avg10" = % of time all tasks stalled
```

`oom_kill 0` is decisive: whatever happened, it was not the OOM killer. Do not
report memory exhaustion without that counter being non-zero.

**Orphans wedge the next build.** Killing a `cargo` leaves its `rustc` children
adopted by init. They keep running, can hold locks, and have been seen blocking
a later build in a *different* `CARGO_TARGET_DIR`. If a build wedges right
after you killed a previous one, that is the first thing to check:

```text
mcp__cas__coordination action=gc_report
```

Orphaned `rustc` shows up as reapable, annotated "build tool with no parent to
report to".

**Reporting is yours; cleanup is the supervisor's.** `gc_report` is read-only —
run it freely. `gc_cleanup force=true dry_run=false` is **not** scoped to your
worktree: it sweeps every worker's worktree on the host, because all workers run
as the same user. Ask the supervisor rather than running it yourself, and say
which pid you want gone. A worker clearing its own wedge with a host-wide kill
is how one worker's recovery becomes another worker's mystery build failure.

**Never select build processes by name to kill them.** `pkill -9 -f rustc` and
`pgrep -x rustc` match another worker's live compile on a shared host, and you
will destroy their build without knowing — this has actually happened here. The
only pid you may signal directly is one you captured yourself from a process you
started (`$!`), and only after confirming its command line.

Note what the fingerprint does and does not buy you: `gc_cleanup` revalidates a
`/proc` start-time fingerprint before signalling, so it cannot hit a *recycled*
pid — but that proves identity, not that the process is unwanted. It is a
protection against killing the wrong process, not against killing the right
process at the wrong time.

### A test run that looks hung: wedged test binaries (GH #114)

The same fingerprint shows up one level down, in the test binaries `cargo test`
runs. A parked binary from an earlier run holds the lock the next run wants, so
the *new* suite prints nothing and looks like a hung test. It is not hung — it
is blocked behind a corpse. This has been seen twice in one epic; one case
burned an hour before anyone looked at the process table, and once the stale pid
was reaped the "hung" suite finished in **0.11s**.

**Look at the test binaries, not at cargo:**

```bash
ps -eo pid,ppid,etime,time,stat,wchan:20,args | grep -F "/target/debug/deps/"
```

Read the same two columns as for a wedged build, plus `wchan`:

| `TIME` (CPU used) | `WCHAN` | Meaning |
| --- | --- | --- |
| climbing | anything | The suite is running. Slow ≠ stuck. Leave it alone. |
| ~0:00 with large `ETIME` | `futex_do_wait` | Wedged. Alive for minutes, no CPU burned, parked on a futex. |

**Confirm it has no children before calling it dead:**

```bash
pgrep -P <pid>      # no output => nothing is running underneath it
```

A wedged test binary is childless. If it *does* have children, it is a live
suite forking helpers — leave it alone and re-read `TIME`.

**Then reap by pid, and only by pid.** `gc_report` is read-only and yours to
run; cleanup is the supervisor's (`gc_cleanup` is host-wide, not scoped to your
worktree), so name the exact pid you want gone:

```text
mcp__cas__coordination action=gc_report
```

**Never select test binaries by name.** `pkill -f cas-` or `pkill -f
"target/debug/deps"` matches another worker's live test run on this shared host
— every worker runs as the same user, and the deps binaries have identical names
across worktrees. This is the same rule as for `rustc` above, and it has the
same consequence: your recovery becomes someone else's mystery failure. The only
pid you may signal directly is one you captured yourself (`$!`) from a process
you started, and only after confirming its command line.

# GH #1286: Prowl task-start timing

Task: `cas-e926` · Issue: [#1286](https://github.com/Richards-LLC/cassy/issues/1286)

Result posted in [issue comment 6307103124](https://github.com/Richards-LLC/cassy/issues/1286#issuecomment-6307103124).
Portable evidence: `art-9285a735` (`cas-e926-evidence.tar.gz`, committed to Cloud).

The original 55-second stall did not reproduce on Prowl. Installed Cassy
`3.67.0` (`acfb0b0`) already contains the known-root bound from
`2519917c36fd213d7e4c6a85e01dac096097d0ca` (`cas-96ce4`). No additional
production change is justified by this investigation.

The October 18 incident occurred before that bound. An unbounded known-root
filesystem read could explain the old stall; the retained issue has no stack or
stage timing that identifies it as the unique historical cause. The measurements
below verify the repaired gate on the affected host, without claiming a universal
deadline for every task-start operation.

## Isolation and inputs

Used SQLite's online backup API with read-only source connections, rather than
copying live WAL database files. Backup times were 0.349 seconds for the Cassy
project store, 0.004 seconds for the host registry, and 0.604 seconds for the
original Gabber project store. All three snapshots passed `PRAGMA quick_check`.
Snapshots and experiment stores live under this worktree's ignored `target/`.

Every subprocess used an explicit copied `CAS_ROOT` and a sibling temporary
`HOME`. `config get lease.default_duration_mins` returned the sentinel `17`
before each experiment. No Cloud credentials, original project configuration,
or factory environment were inherited. Cloud sync and code indexing were disabled.
The temporary host's fresh sweep debounce marker prevented the copied registry
from triggering cleanup on live repositories; every trace confirms that the
opportunistic sweep was skipped. Git probes used `GIT_OPTIONAL_LOCKS=0`.

Only the copied selected task was made Open and unassigned, its terminal outcome
cleared, and its existing copied lease removed. Notes, delivery anchors, work
target, and dependencies were retained. Starts used a temporary standard agent
instead of an operator factory registration, avoiding live worker-checkout
mutation. Thus these runs exercise the real repository-resolution, local lease,
status, and response paths; they do not reproduce the original Cloud/session state.

The Cassy representative was `cas-2b3a5`: 86 note blocks, 53,017 bytes of notes,
and five retained delivery anchors. Its note count ties the highest among the
snapshot's tasks with at least five anchors. The original Gabber task,
`cas-d465a`, now has 88 note blocks, 54,368 bytes, and three retained anchors.
Its current snapshot is not the exact October 18 database state.

## Results

All times exclude MCP initialization and identity lookup. Server tracing used
`RUST_LOG=cas=debug`; Git subprocesses emitted `GIT_TRACE2_EVENT`. SQLite state
was sampled every 10 milliseconds.

| Experiment | Start response | Resolution through branch validation | Final copied state |
| --- | ---: | ---: | --- |
| Cassy representative, real host registry | 0.787719 s | 0.761757 s | InProgress, matching active lease |
| Original Gabber task, real host registry | 0.768174 s | 0.756089 s | InProgress, matching active lease |
| Gabber, registry extended with real worktree/artifact/host roots, `brief=true` | 0.866142 s | 0.851926 s | InProgress, matching active lease |
| Cassy, one unrelated FIFO config root plus healthy root | 1.110572 s | 1.084680 s | InProgress, matching active lease |
| Cassy, six FIFO config roots plus healthy root | 5.009650 s | Refused during scan | Open, no lease, unchanged lease history |

The first two successful requests made 78 Git probes each; the largest measured
Git process took 0.003955 and 0.004845 seconds respectively. Most request time
was the sequential registry/repository-resolution interval. Note/response work
did not approach 55 seconds. The extended run changed both registry contents
and response mode; it is an additional successful case, not a controlled estimate
of either variable's independent cost.

The extended registry added the live worker checkout, Cassy's worktree directory,
`~/.cas/artifacts`, and `~/.cas` to the copied registry. Separate read-only
resolve/stat probes on these paths and `~/.cas/config.toml` took 6–32 microseconds.
No slow real filesystem path was observed.

## Timeout and mutation evidence

The single FIFO forces a blocking configuration read in an unrelated root.
The caller stopped waiting on that root and completed the start in 1.111 seconds.
Six such roots exhausted the shared scan budget, returning MCP error `-32602`:

```text
WORK TARGET REPOSITORY UNAVAILABLE: known-root scan exceeded its 5s budget;
retry after storage access recovers
```

For the refused request, copied task status, assignee, lease absence, and lease
history count matched the pre-call values immediately and again after keeping
the server alive for two seconds. No late task/lease mutation followed the scan
timeout. Successful requests each added exactly one claim history row and ended
with the same agent in task assignee and active lease. The 55-second response
deadline itself was not reached in these experiments.

The first six-FIFO capture failed in the Python observer: its repeated SQLite
context managers committed transactions but did not explicitly close connections,
eventually exhausting file descriptors. That failed capture was retained. After
changing the observer to `contextlib.closing`, the same six-FIFO experiment
completed and supplied the 5.009650-second refusal above. This was a harness
error, not evidence of a Cassy database failure.

## Existing implementation and coverage

`cas-cli/src/mcp/tools/core/task/repo_context/candidate_probe.rs` bounds waiting
to one second per root, shares pending probes, negatively caches unavailable
results for 30 seconds, and caps pending threads at 64. Blocking filesystem calls
remain on isolated threads; the bound applies to caller waiting, not termination
of a kernel-blocked filesystem call. `candidate_paths` in `repo_context.rs` uses
a shared five-second deadline. This does not establish bounds on unrelated
operations such as host-registry opening or explicit binding validation.

The existing regression is
`repo_context::tests::unreachable_known_roots_are_bounded_and_negatively_cached`,
added in `2519917c3`; it exercises blocked Git and FIFO configuration reads.
No Rust tests were run for this no-code investigation, per supervisor direction.
Existing start-cancellation and terminal-receipt coverage was inspected, not
re-executed. The runtime evidence here directly covers a root timeout, scan-budget
refusal without mutation, and successful lease/status consistency.

## Evidence and conclusion

Durable evidence is under the project-scoped task artifact directory:

```text
~/.cas/artifacts/cassy-ee10031b42e3c2024023c076dfb2edd8118e39b8baee6f0832367b9e8ef4c205/cas-e926/
```

`time_snapshot.py` records sentinel checks, traces, state transitions, and
per-case `result.json`; `summarize.py` validates those receipts and emits
`summary.json`. `real-path-metadata.json` contains the filesystem observations.
No database snapshot or credential file is included in the portable evidence.

The known-root gate is already bounded and behaves promptly on current Prowl
paths. The original timeout is classified as pre-bound behavior compatible with
the formerly unbounded probe, with its precise historical cause unconfirmed.
The investigation closes with measured evidence and no speculative source fix.

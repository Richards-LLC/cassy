# Estimates from measured history

`cas ledger estimate pipeline merge_queue --instance <sha>` records a promise
for that exact GitHub head. Stages are `train_tip_suite`, `merge_queue` and
`release_publish`. `cas ledger estimate task <id>` and `epic <id>` rank closed
tasks in that project by type, priority, risk, files touched, worker model and
lease history. `release vX.Y.Z` uses measured full release-cycle receipts.
`--recipient client` records a client promise; the default is operator.
`--runs 20` bounds the reference class. `--preview` reads without issuing a
promise. JSON includes the evidence count, five reference identities and measured
durations, and task-reference metadata. Fewer than five references is explicitly
uncalibrated. No references means no numeric range and no invented promise.

MCP exposes the same estimator through `system action=estimate` with
`query="pipeline:merge_queue:<sha>"`, `task:<id>`, `epic:<id>` or
`release:<version>`, and optional `recipient=client`. Factory status and the
release command show measured stage ranges. The release command also records its
release promise; missing telemetry never blocks preparation or publication.

Pipeline samples use GitHub run creation through the last completed job, including
runner acquisition. They do not use `updated_at`, commit author time or multiply a
single host's build duration. Train samples are successful CI push runs on
`epic/release-train-*` with full-suite jobs. Queue samples are successful
`merge_group` CI runs; publication samples are successful Release push runs.
Full release samples use the receipt's `REQUEST_TO_PUBLISHED_SECS`; publication
alone is a separate stage. Task references use first claim through close when a
retained lease exists, otherwise creation through close, with that absence shown.
The nearest-rank p50 and p90 are computed over at most the requested recent runs
or closest tasks. Missing model/files/lease metadata remains visible, not guessed.
These are historical full-duration references; a promise issued mid-flight is
settled against the remaining elapsed time after it was issued.

Successful task close and observed pipeline completion settle outstanding promises.
The existing GitHub poll reuses its completed-run response and reads job times
only for an outstanding matching promise; release receipt recording settles the
release. Outcomes link the immutable estimate event id and actual elapsed minutes.
A hit means actual is at most p90. Early completion is a hit with
`below_typical=true`; only exceeding p90 creates a friction event. Repeated
completion/backfill never duplicates a fact. `cas ledger report` shows completed
estimates, hit rate, early completions, misses with recorded reason codes, and
per-release history. Unexplained overruns are `above_p90`, not an invented cause.

## Backfill after deployment

The supervisor runs this after the release containing this change installs on
soundwave and prowl and Cloud issue Richards-LLC/petra-stella-cloud#263 deploys.
The worker proves the operation on an isolated local fixture; live fleet counts
are the supervisor's post-install proof.

For each registered project checkout, from that checkout with factory root
redirects removed, run:

```sh
env -u CAS_ROOT -u CAS_CLONE_PATH cas --json ledger backfill --days 3650 --sync
```

On prowl, non-login SSH needs the local binary directory:

```sh
ssh prowl 'export PATH="$HOME/.local/bin:$PATH"; cd /path/to/project; env -u CAS_ROOT -u CAS_CLONE_PATH cas --json ledger backfill --days 3650 --sync'
```

Use the host's `cas known-repos` inventory; include every project with its own
store, once per canonical root. Repeat safely. Backfill derives task-duration
check rows, retained verification/queue history and bounded GitHub run/job facts;
it never invents historical estimates or claims that an old promise was made.
Github history is bounded to twenty runs per requested workflow/event page;
missing permissions or workflows are reported as skipped. Other local backfill
continues when GitHub evidence is unavailable.

Keep per-project JSON, local `quality_events` counts by kind before/after and the
Cloud-confirmed counts by project/machine. A sync round is bounded: repeat
`--sync` until no rows push and no rows are refused; a null Cloud summary means
not logged in, not success. Check `capability_absent` and refused counts. The
extended lane for friction/estimate/outcome holds its cursor on unsupported kinds
and retries after a cooldown or an explicit sync; older event pushes and pulls
continue. Once Cloud accepts the kinds, event-id idempotency safely replays the
retained rows. No credentials or free-form local notes are sent as evidence.

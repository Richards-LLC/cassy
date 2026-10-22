# Managed review scratch

| Field | Contract |
| --- | --- |
| First two lines | `scratch-checkout` prints the absolute checkout path alone so a supervisor can use it directly; failures explain the rejected owner or commit. |
| Scannable | GC adds one managed review category with task, disposition and bytes; its default context states total reclaimable bytes and pending deletion bytes, while large unmanaged directories stay review-only. |
| Readable | The existing bounded GC projection fits 79 columns; complete paths remain in `--full` and JSON. |
| Machine output | GC's existing single JSON document includes `REVIEW_SCRATCH_STATUS_JSON` with schema_version, rows, reclaimed_bytes, pending_reclaim_bytes and total/per-row reclaimable_bytes; target-cache/admission reports also expose pending_reclaim_bytes without treating queued bytes as free space. Checkout creation returns one raw path, or a single `{path, task_id, dispatch_id, head}` object under `--json`. |
| Omitted | No build output appears during checkout creation. GC never automatically adopts unmanaged directories. |

Review output is ASCII, monochrome and append-only. The existing GC renderer
owns truncation and pagination. Checkout creation prints a filesystem path,
which is consumed as data rather than as a shell command.

## Reclamation safety correction

GC retains edited tracked files and untracked reviewer notes. Existing source
checkouts use non-force Git removal; only a vanished checkout permits forced
registration cleanup. Successful reuse refreshes the two-hour idle clock, with
creation-time fallback for legacy records. Unmanaged size inventory stops at
1 GiB and prints an explicit lower bound. Correction proof and the same-command
before/after real-build ledger are stored in task artifacts under `correction/`.

Finished review targets (resolved, timed out or invalidated), or terminal tasks' targets without an active dispatch,
are reclaimed on verdict/close or the next guarded GC pass (at most 16 per pass).
Only source and its registration retain the two-hour idle grace period. A young
checkout's `reclaimable_bytes` counts its build target; `bytes` remains the total
inventory. Pending/claimed dispatches, held leases and live output remain protected.

Verdict/close responses wait only for ownership-checked atomic target handoff to
the private checkout's `trash/target`, with the owner marker written first.
Measurement and unlinking run in the background under the existing build lane;
GC/admission recover interrupted trash. A project collector lock serializes bulk
deletion, and pending trash remains counted until it is actually removed.
Source and ownership records remain available while trash still needs collection.

## Original delivery critique

Candidate: `cas 3.71.0 (3d88af2 2026-10-22)`, supervisor-built revision
`3d88af29a725d9f2bd67dcf99072fa4c4c61f21b`. The five-cell real-build walk
passed creation/reuse, terminal collection with Git registry removal,
active/young/open-file retention, unmanaged inventory/preservation and unnamed
verifier-test refusal. Full source/cap behavior also has targeted Rust evidence.

| Receipt | Runs | Failures | Warnings |
| --- | ---: | ---: | ---: |
| `terminal-qa: PASS scratch-checkout-help` | 11 | 0 | 0 |
| `terminal-qa: PASS review-gc-report` | 11 | 0 | 0 |

The help gate executes the real candidate in each profile. The report gate
replays its captured `report_text`; the real CLI JSON document was separately
parsed and checked during the walk. Both cover 80/120 columns on four palettes,
pipes, NO_COLOR and C locale. Captured text and palette HTML were inspected.
The creation command's raw path is machine data: it stays intact even when a
filesystem path exceeds terminal width, and `--json` supplies its object form.

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | GC names the inventory count and cleanup-help action first; command help shows its purpose and required owner arguments. |
| Fit | 4 | The existing bounded GC projection keeps complete values in full/JSON; required usage arguments fit on two lines. |
| Craft | 4 | The first help capture exposed a 101-column usage line; the corrected real build passes every 80-column palette. |
| Theme safety | 5 | Monochrome ASCII output passes all palettes, NO_COLOR and C with no allowlist. |
| Machine contract | 5 | The walk parses creation and GC JSON as single objects and reads the Git registry after collection. |

The original failed help profiles remain in the task proof bundle. Thresholds
(two hours and 1 GiB apparent bytes) are explicit in output and docs. The CLI
captures do not claim a full Rust assembly or a non-Linux native process walk.

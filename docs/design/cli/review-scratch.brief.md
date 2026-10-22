# Managed review scratch

| Field | Contract |
| --- | --- |
| First two lines | `scratch-checkout` prints the absolute checkout path alone so a supervisor can use it directly; failures explain the rejected owner or commit. |
| Scannable | GC adds one managed review category with task, disposition and bytes; large unmanaged directories are explicitly review-only. |
| Readable | The existing bounded GC projection fits 79 columns; complete paths remain in `--full` and JSON. |
| Machine output | GC's existing single JSON document includes `REVIEW_SCRATCH_STATUS_JSON` with schema_version, rows and reclaimed_bytes. Checkout creation returns one raw path, or a single `{path, task_id, dispatch_id, head}` object under `--json`. |
| Omitted | No build output appears during checkout creation. GC never automatically adopts unmanaged directories. |

Review output is ASCII, monochrome and append-only. The existing GC renderer
owns truncation and pagination. Checkout creation prints a filesystem path,
which is consumed as data rather than as a shell command.

## Critique

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

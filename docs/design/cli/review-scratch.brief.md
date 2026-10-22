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

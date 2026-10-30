# Factory GC cleanup

| Field | Contract |
| --- | --- |
| First two lines | Say cleanup completed and show the command for reviewing its safety tiers; counts below distinguish metadata changes from preview-only cache/process work. |
| Scannable | Keep the existing cleanup counters, one count per inventory category, and at most three largest byte-counted items in each category, fitted to 79 columns. |
| Readable | Short reasons explain retained candidates; complete paths, status payloads and errors remain available under `--full`. |
| Machine output | `--json` returns one versioned document with `report_text`, lossless `full_report` and parsed `machine_status`; no progress appears on either stream. |
| Omitted | Raw status JSON and unbounded path lists stay behind `--full` or `--json`; human cleanup emits a plain stderr heartbeat while it runs. |

Deletion gates remain unchanged: baseline metadata cleanup, force-only legacy
tiers, and explicit force plus no-dry-run for caches/processes/journey recordings.

design: DESIGN.md (inherits hub-web/DESIGN.md)

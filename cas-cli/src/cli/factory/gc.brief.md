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

## Critique

terminal-qa: PASS gc-cleanup · 12 runs · 0 fail · 0 warn · 0 allowed.
Evidence: task `cas-d26b`, `terminal-qa/report.json`, candidate `27087c279`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | stdout begins with the completion verdict, category count and cleanup-tier help; stderr identifies ongoing work. |
| Fit | 4 | 240 review-only directories produce 45 lines and three samples; full detail remains opt-in. |
| Craft | 4 | Maximum width is 79 cells; paths shorten without hiding byte counts, and context stays inside its category. |
| Theme safety | 5 | Four palettes, C locale and NO_COLOR pass; light/dark 80-column captures reviewed. |
| Machine contract | 5 | One JSON document, zero stderr, complete status and full report; no consent gate changes. |

Measured against the isolated real candidate on 2026-10-30. The source-bound
ledger retains the initial public-dispatch refusal and grouping-control captures.

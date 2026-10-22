# Factory GC report

| Field | Contract |
| --- | --- |
| First two lines | State the number of inventory categories to review, then offer `cas factory gc cleanup` as the safe preview step. A completed inventory is not a promise that every item is reclaimable. |
| Scannable | Show project metrics, one category/count row, a short context and up to three sample items per category; preserve unavailable and review-only findings. |
| Readable | Wrap prose below 80 columns with hanging indentation, use ASCII punctuation and escaped dynamic values, and shorten oversized tokens without splitting them. |
| Machine output | `--json` returns one object with `schema_version`, bounded `report_text`, complete `full_report`, and `machine_status` containing typed status objects, including `TARGET_CACHE_STATUS_JSON`; no banner or ANSI escapes surround it. MCP `json=true` returns the same text document. |
| Omitted | Raw status JSON and complete long values are absent from the default summary; `--full` or MCP `full=true` returns the original report. Existing MCP `limit`/`offset` pages retain original-line offsets and byte caps. |

The CLI forwards its global `--json` and `--full` options to the shared handler.
JSON plus full selects complete JSON; explicit pagination is a separate view.
The shared human output uses 79 columns at both 80 and 120 column terminals,
with no locale, color, terminal-width or pipe-dependent rendering differences.
Cleanup safety and collection policy remain unchanged.

## Critique

Pending executed renderer and exact-build terminal QA. No completion claim.

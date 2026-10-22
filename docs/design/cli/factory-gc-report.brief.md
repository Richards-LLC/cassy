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

Reviewed the supervisor-built `5190b3d95aea6aa258eb5b94547e868ca0fe01ca`
candidate (`cas 3.71.0`, SHA-256
`41f607f96cfcf0b7b6ec95720a1186ff0f8aeb00e58f1e9da70910ef3b249c9f`).
Evidence lives under the project-scoped `cas-a0f8` task artifacts:
`LEDGER.md`, `terminal-qa/`, and `captures/`.

| Receipt | Runs | Failures | Warnings |
| --- | ---: | ---: | ---: |
| `terminal-qa: PASS gc-report` | 12 | 0 | 0 |
| `terminal-qa: PASS gc-report-C` | 11 | 0 | 0 |
| `terminal-qa: PASS gc-help` | 11 | 0 | 0 |
| `terminal-qa: PASS gc-help-C` | 11 | 0 | 0 |

Both 80 and 120 columns were exercised under UTF-8 and C, on all four
palettes; pipe, NO_COLOR and JSON controls also passed. No allowlist was used.
The same fixture/scripts with the prior real-build candidate reproduced 64
report overflows and the GC help's one C-locale Unicode failure plus eight
word-split warnings. That baseline predates the exact merge base; the executed
renderer regression also failed before the implementation and passed after it.

| Dimension | Score | Observed evidence |
| --- | ---: | --- |
| Hierarchy | 4 | Category count and cleanup action occupy the first two lines; full/JSON commands and report size follow inventory. The preview wording needs the baseline-tier qualification tracked in cas-ae91. |
| Fit | 4 | Categories collapse into counts and at most three samples; long paths have explicit full/JSON escapes. Existing project metrics remain individually visible. |
| Craft | 4 | Dark/light 80-column screenshots show readable hanging context indentation, whole tokens and no horizontal clipping; both HTML blocks measure 650/650 client/scroll width. |
| Theme safety | 5 | Four palettes, C and NO_COLOR pass without color-dependent meaning; the body uses the terminal default foreground. |
| Machine contract | 5 | Real MCP and CLI summary bytes match with one live inventory; JSON is one complete typed document, full retains raw status, and incompatible paging flags refuse. |

Adjacent root help still chooses Unicode wordmark art in a styled C-locale TTY;
its failed gate is retained and tracked as cas-b596. The inherited authored
U+2014 text is removed, and the required GC help matrices pass.
The default cleanup previews cache/process/journey tiers while still performing
baseline stale metadata cleanup; cas-ae91 tracks clarifying the report's action
label. Cleanup policy is unchanged. The ledger records these limitations and
the observer/browser setup corrections without treating them as passing checks.

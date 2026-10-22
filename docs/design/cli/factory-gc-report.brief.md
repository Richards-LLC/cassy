# Factory GC report

| Field | Contract |
| --- | --- |
| First two lines | State the number of inventory categories to review, then direct the reader to `cas factory gc cleanup --help` to review cleanup tiers. Default cleanup still removes stale metadata; cache/process/journey deletion has separate preview controls. A completed inventory is not a promise that every item is reclaimable. |
| Scannable | Show project metrics, one category/count row (excluding informational notes), a short context and up to three sample items per category; preserve unavailable and review-only findings. |
| Readable | Wrap prose below 80 columns with hanging indentation, use ASCII punctuation and escaped dynamic values, and shorten oversized tokens without splitting them. |
| Machine output | `--json` returns one object with `schema_version`, bounded `report_text`, complete `full_report`, and `machine_status` containing typed status objects, including `TARGET_CACHE_STATUS_JSON`; no banner or ANSI escapes surround it. MCP `json=true` returns the same text document. |
| Omitted | Raw status JSON and complete long values are absent from the default summary; `--full` or MCP `full=true` returns the original report. Existing MCP `limit`/`offset` pages retain original-line offsets and byte caps. |

The CLI forwards its global `--json` and `--full` options to the shared handler.
JSON plus full selects complete JSON; explicit pagination is a separate view.
The shared human output uses 79 columns at both 80 and 120 column terminals,
with no locale, color, terminal-width or pipe-dependent rendering differences.
Cleanup safety and collection policy remain unchanged.

## Critique

Reviewed the supervisor-built `29e13c2feb4a0d780d9cc935f78016933d601421`
candidate (`cas 3.71.0`, SHA-256
`48bd7d39222ccba1f290249c6b53c2b3ee99d7cc362753e11ab34a4e18656872`).
Evidence lives in the project-scoped `cas-ae91` artifacts: `LEDGER.md`,
`terminal-qa/`, `captures/`, and the dark/light terminal screenshots.

| Receipt | Runs | Failures | Warnings |
| --- | ---: | ---: | ---: |
| `terminal-qa: PASS gc-report` | 12 | 0 | 0 |
| `terminal-qa: PASS gc-report-C` | 11 | 0 | 0 |
| `terminal-qa: PASS gc-cleanup-help` | 11 | 0 | 0 |
| `terminal-qa: PASS gc-cleanup-help-C` | 11 | 0 | 0 |

The gates cover 80 and 120 columns under UTF-8 and C, all four palettes,
pipe and NO_COLOR; the report also passes the JSON control. No allowlist was
used. The owned dirty-worktree fixture now shows eight inventory categories
and eight category rows, with the WIP preservation note rendered separately.
The same fixture and report assertion on the earlier `5190b3d95` real build
failed: it showed nine categories, counted the note as a category and called
default cleanup a preview. That binary predates this task's merge base; the
committed renderer regression independently ran red before this fix.

| Dimension | Score | Observed evidence |
| --- | ---: | --- |
| Hierarchy | 4 | Lines one and two show eight categories and the copyable `cas factory gc cleanup --help` action; that help explicitly describes baseline metadata mutations. |
| Fit | 4 | Eight category rows match the header, with three samples at most; the informational WIP note is preserved without a category count. |
| Craft | 4 | Dark/light 80-column captures show whole action tokens and hanging context indentation; all four UTF-8/C screenshot blocks measure 650/650 client/scroll width. |
| Theme safety | 5 | Four palettes, C and NO_COLOR pass with ASCII text and no color-dependent meaning. |
| Machine contract | 5 | CLI and MCP show the same action/count; JSON and full retain the complete raw statuses, and original-line paging and incompatible-view refusals pass. |

The six real-build ledger cells pass, including JSON+full and the outside-project
refusal. Startup pressure guidance and documentation point to the explicit
full/JSON status views. Collection and cleanup policy are unchanged; nonempty
baseline cleanup mutations were not induced during this wording pass.
The first screenshot attempt through the Playwright MCP failed at the local
Chrome sandbox setup; local Playwright rendered the same static gate HTML
successfully, and the resulting dark/light images were inspected.

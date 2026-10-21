# cas qa fixtures --sweep

| Field | Contract |
| --- | --- |
| First two lines | The first line gives the cleanup verdict and counts; retained debt adds one retry command on line two. |
| Scannable | One compact summary shows cleaned handles and orphan debt, without one row per successful teardown. |
| Readable | The retry command is `cas qa fixtures --sweep`; `--json` provides each retained cleanup ID and cause for investigation. |
| Machine output | `--json` emits one object with numeric `cleaned` and `orphan_debt: [{cleanup_id, error}]`, with no human banner. |
| Omitted | Handles, recipe outputs, adapter stdout and per-call transcripts stay off the human screen; JSON includes only cleanup IDs and sanitized errors. |

The output uses ASCII marks and no colors or cursor controls, so pipes, narrow
terminals, light/dark themes and C locales carry the same meaning. Teardown
debt is reported without modifying any suite verdict. A sweep with retained
debt exits unsuccessfully after printing its report.

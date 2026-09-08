# Brief: `cas cloud sync`

| Field | Sentence |
| --- | --- |
| First two lines | Whether the pull completed, how many errors need attention, and the first concrete sync error when the pull is incomplete. |
| Scannable | One verdict line with the error count and sync counts, followed by one indented row per pull error in non-verbose mode. |
| Readable | Each error remains the sync layer's concrete entity and cause; `--verbose` keeps the existing detailed count rows and conflict diagnostics. |
| Machine output | `--json` remains the existing single sync receipt; human error rows are not emitted into the machine document. |
| Omitted | Successful per-kind rows and conflict details stay collapsed in the default view and remain available under `--verbose`. |

## Rendering decisions

- An incomplete pull keeps the warning verdict on the first line and now prints each
  pull error immediately below it, so a user can identify the failed entity without
  rerunning with `--verbose`.
- Error rows use the existing muted formatter and four-space hanging indent; the
  summary's warning mark and error count remain the only verdict signals.
- The JSON path and successful summary path are unchanged.

## Critique

terminal-qa: PASS cas-280c-cloud-sync · 11 runs · 0 fail · 0 warn · 17 allowed · /home/pippenz/.cas/artifacts/cas-280c/terminal-qa/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | `Pull incomplete · N errors` remains the first line and the first error is the second line. |
| Fit | 4 | The default view adds one compact row per failure while successful counts remain collapsed. |
| Craft | 4 | Errors share the existing four-space indent and formatter; no new columns or wrapping are introduced. |
| Theme safety | 5 | The new muted rows are readable without color; the receipt allowlists 17 pre-existing spinner/verdict findings outside this task. |
| Machine contract | 5 | `--json` is unchanged and the human-only rows are not part of the sync receipt. |

Scored by the worker on 2026-09-08; floor holds. Allowlist: `/home/pippenz/.cas/artifacts/cas-280c/cas-cloud-sync-allowlist.json`.

# cas update phase spacing

| Field | Contract |
| --- | --- |
| First two lines | Each update phase names its work and shows a separate working or completed status. |
| Scannable | The mark, counter, label, status and elapsed time remain distinct columns, with at least one space after every label. |
| Readable | A label that fills or exceeds its padded column stays readable without merging into the status word. |
| Machine output | The existing single update JSON document is unchanged; progress rows appear only in human output. |
| Omitted | No new progress fields or timing details are added; this change only preserves the label/status boundary. |

## Critique

`terminal-qa: PASS cas-f1354 · 12 runs · 0 fail · 0 warn · 0 allowed`

Supervisor candidate `ff0f0c5d767def46fb79bb01ee4c37ad3957b8aa`,
`cas 3.72.0 (ff0f0c5 2026-10-22)`, passed the native update terminal gate
with `--json-flag --json` (supervisor receipt #543166). The 80-column result
keeps the complete user-level label, a space, `done`, and elapsed milliseconds.
The real piped capture also preserves the boundary before `working`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The first row names the phase and its done verdict before the builtin detail. |
| Fit | 5 | Both full-label working and done rows fit 80 columns; all 80/120 captures pass. |
| Craft | 4 | A minimum label/status gap preserves distinct columns, with elapsed time still right-aligned. |
| Theme safety | 5 | Four palettes, NO_COLOR and C locale pass with no findings. |
| Machine contract | 5 | JSON remains one document; piped progress appends start/result rows without redraws. |

Scored by young-otter-20 on 2026-10-22 against supervisor native captures.
The initial artifact probe incorrectly counted the same-label section heading
as a third step row and exited 1. Its corrected selector matches `[1/1]` rows;
replay against the unchanged real CLI capture passes both status boundaries
and the width check. The original probe and its failure are retained in the
proof bundle; this is an evidence-selector correction, not a product change.

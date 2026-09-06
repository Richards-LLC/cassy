# Brief: `cas update` (human mode)

| Field | Sentence |
| --- | --- |
| First two lines | Whether anything is pending — a newer binary, unapplied migrations — or, after a refresh, whether every project converged. |
| Scannable | `--check`: two labelled rows (Binary, Schema). After a refresh: the verdict line with counts and the project table with one mark per phase. |
| Readable | Per-project detail lines, only for a project whose phase was not `✓`. |
| Machine output | `--json`: one receipt document (`--check` emits the version/migration object; a refresh emits the combined receipt); progress never appears. |
| Omitted | Successful phases' transcripts and the `Run … to …` sentences: the remedy is the command itself under `→`. |

## Rendering decisions

- `--check` opens with `✓ up to date · Cassy 3.17.2 · schema v254` or
  `⚠ update available · 3.17.2 → 3.18.0`, then `Binary` and `Schema` rows, then one remedy.
- The refresh banner became a verdict line: `✓ complete`, `⚠ N not refreshed`, or
  `✗ N projects failed`, with the unchanged count grammar as its detail.
- The `Current version: / Latest version:` pairs, each in accent colour, are gone; versions
  are plain text in the row.

## Critique

Before (build `eda3dfd1`): `terminal-qa: FAIL cas-update-check · 12 runs · 33 fail · 0 warn` — 32 contrast, 1 unicode-without-fallback.

After: `terminal-qa: PASS cas-update-check · 12 runs · 0 fail · 0 warn · 0 allowed · docs/design/cli/captures/after/cas-update-check/report.json`

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | verdict first; the remedy is the only line after the rows |
| Fit | 4 | two rows for a two-fact answer; the refresh table still carries a free-text note column |
| Craft | 4 | fixed label column; `→` between versions reads as a change |
| Theme safety | 5 | marks only; four palettes pass |
| Machine contract | 5 | `--check --json` is one object |

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
- A failed post-swap refresh names each failing project and its failed phase, then gives one
  copyable remedy (`cas update --all-projects`); the old "outcome is unknown" wording remains
  only when the child truly produced no receipt.
- The `Current version: / Latest version:` pairs, each in accent colour, are gone; versions
  are plain text in the row.

## Critique

Before (build `eda3dfd1`): `terminal-qa: FAIL cas-update-check · 12 runs · 33 fail · 0 warn` — 32 contrast, 1 unicode-without-fallback.

After: `terminal-qa: PASS cas-update-check · 12 runs · 0 fail · 0 warn · 0 allowed · docs/design/cli/captures/after/cas-update-check/report.md`

Post-swap failure review: `terminal-qa: PASS cas-update · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-a461/terminal-qa/report.json`; the plain-mode
failure test keeps the first line actionable by naming the failed project and the one rerun command.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | verdict first; post-swap failures name the project before the one rerun command |
| Fit | 4 | two rows for a two-fact answer; partial failures retain structured project evidence |
| Craft | 4 | fixed label column; failure guidance uses one copyable remedy |
| Theme safety | 5 | marks only; four palettes pass |
| Machine contract | 5 | `--check --json` is one object |

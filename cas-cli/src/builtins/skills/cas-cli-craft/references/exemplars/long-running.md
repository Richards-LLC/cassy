# Exemplar: long-running command

A multi-phase command (`cas update` refreshing every local project). It runs for seconds to
minutes; the reader wants to know what phase it is in, and afterwards, one receipt.

## Brief

| Field | Sentence |
| --- | --- |
| First two lines (after) | Whether the refresh converged, how many projects, how many failed, how long. |
| Scannable | During: one status line that redraws. After: a project table with one mark per phase. |
| Readable | Per-project detail rows, shown only for a project whose phase was not `✓`. |
| Machine output | `--json`: one receipt document; progress goes nowhere — a script wants the receipt, not a spinner. |
| Omitted | Successful phases' transcripts (`--verbose`). |

## Render at 80 columns — TTY, mid-run

```
→ [2/2] Refreshing all local Cassy projects · 7 of 29 · 12s
```

The line is rewritten in place (`\r` + erase-to-end) once per project; finished phases are
printed once and stay. Under thirty seconds nothing else appears; past thirty, the line names
the project it is waiting on.

## Render at 80 columns — piped or `--json`

Piped: each phase transition appends one line with a timestamp and nothing redraws.

```
17:41:02 [1/2] Updating Cassy binary … 0.9s
17:41:03 [2/2] Refreshing all local Cassy projects … 41s
```

`--json`: silence until the single receipt document.

## Render at 80 columns — TTY, finished

```
✓ complete · Cassy 3.17.2 · 29 projects refreshed · 0 failed · 41.3s
  project                 migr  index  skills  member  cloud  note
  cas-src                 ✓     ✓      ✓       ✓       ✓
  gabber-studio           ✓     ✓      ✓       ✓       –      not linked
  penguinz                ✓     ✓      ⚠       ✓       ✓      2 skills conflict
    penguinz details:
      [WARN] skills: .claude/skills/cas/SKILL.md is locally modified
  [OK] user-level store: 3 built-ins refreshed
```

## Annotations

1. **Verdict** — `✓ complete` when nothing failed, `⚠ 3 not refreshed` when stores were
   skipped, `✗ 2 projects failed` otherwise; the detail keeps the exact counts a script greps.
2. **One status line while running**, appended lines when piped — never a spinner in a log file.
3. **Table** — marks per phase, the note column last and free-text, header once in bold. Rows
   never wrap: at 80 columns the project column truncates with `…` and `--full` is named
   beneath the table.
4. **Detail only on demand** — a project with all marks `✓` gets one row; a project with a
   `⚠` gets its detail lines indented beneath it.

## Score

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | verdict line first after the run; table is evidence |
| Fit | 5 | redraw on TTY, append when piped, one document under `--json` |
| Craft | 4 | note column is free text and can run long at 80 |
| Theme safety | 5 | marks only |
| Machine contract | 5 | receipt document names which binary ran the phases |

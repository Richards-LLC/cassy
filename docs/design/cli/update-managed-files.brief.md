# cas update managed-file repair

| Field | Contract |
| --- | --- |
| First two lines | The managed-files receipt says current, committed, staged or conflicts, with the pending action where needed. |
| Scannable | One compact managed-files line per project; doctor has one managed file drift finding. |
| Readable | Staged changes name the existing staged work as the reason; conflicts remain untouched and JSON names their paths. |
| Machine output | Update adds managed_files with outcome, untracked, staged, commit and conflicts to its existing JSON receipt; doctor returns its existing checks shape. |
| Omitted | Builtin per-path inventories remain in JSON and existing verbose sync details; no push or unrelated project changes enter the repair commit. |

## Critique

The conflict receipt is the scoped terminal surface. A preflight conflict now
returns its own verdict without a surrounding success message, and the sweep
keeps that verdict as a warning. Existing detailed successful-sync output is
outside this renderer change.

`terminal-qa: PASS update-managed-files · 12 runs · 0 fail · 0 warn · 0 allowed`

Captures: task cas-6718 artifacts, `terminal-qa/report.md` (actual native binary,
80/120 columns, four palettes, pipe, C locale, NO_COLOR and JSON).

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | First line says “1 conflicts preserved” and names `cas update --sync --json`. |
| Fit | 5 | One compact conflict row; JSON names the preserved path. |
| Craft | 4 | The 70-column row fits at 80 and uses the same managed-files grammar as other outcomes. |
| Theme safety | 5 | Default foreground and ASCII text pass every palette and monochrome capture. |
| Machine contract | 5 | JSON is one document; human pipes have no redraw or cursor controls. |

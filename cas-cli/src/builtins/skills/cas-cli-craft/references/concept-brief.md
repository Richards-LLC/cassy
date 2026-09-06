# Concept brief for a command

Five fields, one specific sentence each, committed as `<command>.brief.md` beside the renderer.
A field that would fit any command is not filled.

| Field | Question it answers | Example (`cas doctor`) |
| --- | --- | --- |
| **First two lines** | What must the reader know before scrolling? | "The store is healthy or it is not, and how many findings need a hand." |
| **Scannable** | What is read by eye in a sweep — rows, marks, numbers? | "Group rows: one line per healthy group; one row per finding with its glyph." |
| **Readable** | What is read word by word, and only when the reader chooses to? | "Cause and remedy of each finding; timings under `--verbose`." |
| **Machine output** | What does `--json` return, and what never appears there? | "One array of checks with `status`, `message`, `remediation`, `duration_ms`; no banner, no colour." |
| **Omitted** | What was deliberately left off the screen, and where it lives? | "Per-check timings and the slow-phase table; both under `--verbose`." |

## Writing the fields

- *First two lines* is a verdict and a count, not a title. "cas doctor" is the prompt the
  reader typed; repeating it is the weakest opening a command can have.
- *Scannable* names the shape (rows, columns, marks) and the reading order; *readable* names
  the prose and what unlocks it.
- *Machine output* is a contract: field names, one document, stable order. If a human field
  has no machine twin, say so here; the reverse is usual and fine.
- *Omitted* is where ambition shows. The 14-line paragraph you did not print is the brief's
  proudest line.

## Critique section

After the gate and the rubric run, append:

```markdown
## Critique
terminal-qa: PASS cas-doctor · 11 runs · 0 fail · 0 warn · 0 allowed · .cas/artifacts/terminal-qa/cas-doctor/report.json
| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | "⚠ 2 warnings" is line one; the remedy is line four |
| Fit | 4 | groups collapse to one line when healthy |
| Craft | 4 | numbers right-aligned; one unit header |
| Theme safety | 5 | 4 palettes pass; NO_COLOR reads the same |
| Machine contract | 5 | `--json` is one array; stderr carries the banner |
Scored by <who> on <date>; floor holds.
```

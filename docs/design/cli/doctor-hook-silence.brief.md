# Doctor hook silence diagnostic

| Field | Contract |
| --- | --- |
| First two lines | The existing doctor verdict includes a warning when recent factory turns were observed without UserPromptSubmit. |
| Scannable | One `prompt hook` row shows the number of missed prompts across recently active sessions. |
| Readable | The finding identifies UserPromptSubmit and directs the operator to restart Claude; fallback delivery continues in the current session. |
| Machine output | The existing doctor JSON check carries `name`, `status`, and `message`; no new output envelope or ANSI is introduced. |
| Omitted | Raw prompts, session identifiers, and transcript bodies stay out of the diagnostic; only bounded delivery receipts are counted. |


## Critique

terminal-qa: PASS cas-doctor-hook-silence · 12 runs · 0 fail · 0 warn · 0 allowed

The gate used `--escape-flag --verbose`, matching doctor's existing footer,
and `--json-flag --json`. Full receipts and captures are in the task's durable
`terminal-qa-final/` artifact directory. The warning-row capture at 80 columns:

```text
  ⚠ prompt hook  UserPromptSubmit hook silent for 1 prompts; restart Claude
```

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The warning participates in the first-line count and names the failed channel. |
| Fit | 5 | All 80-column captures fit; the complete new finding occupies one line. |
| Craft | 4 | One count and one remedy use the existing doctor row vocabulary. |
| Theme safety | 5 | Four palettes, NO_COLOR, and C locale pass without exceptions. |
| Machine contract | 5 | One valid JSON document carries the same check and finding. |

Scored by clever-jay-9 on 2026-09-08. The initial gate used its default
`--full` escape flag and rejected unrelated truncations; using doctor's actual
`--verbose` escape resolved all 12 findings without renderer changes.

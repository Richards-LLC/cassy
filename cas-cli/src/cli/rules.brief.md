# Rule reference navigation

| Field | Contract |
| --- | --- |
| First two lines | Give the number of references, then the command flag for complete machine output. |
| Scannable | One sorted path and line per comment, followed by its scope. |
| Readable | Scope stays JSON-quoted so embedded quotes and newlines remain unambiguous. |
| Machine output | `--json` emits one array of `{rule_id, path, line, scope}` objects, with repository-relative paths and one-based lines, no banner. |
| Omitted | No rule mutation, credential access, untracked/ignored-file scan, symlink traversal or guessed stakeholder attribution; complete values are available with `--json` or `--full`. |

The command works from any subdirectory of a Git checkout without Cassy login or initialization. It scans tracked working-tree text, including unstaged edits. An invalid marker for the requested rule fails with its path and line; unrelated rule ids do not affect the query.

## Critique

Native build: `9b080b6bc0ce1e8ed756b896b593873c69192bd4`. Portable captures and receipts: `art-22f80445` on task `cas-4f87`.

This initial critique covers the unchanged CLI renderer. After integration with stakeholder provenance, the task's final evidence ledger records a new native build and repeated terminal checks.

```text
terminal-qa: PASS rule-refs · 12 runs · 0 fail · 0 warn · 0 allowed
terminal-qa: PASS rule-refs-invalid · 11 runs · 0 fail · 0 warn · 0 allowed
```

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Count first, complete-value escape second; the error names the failure before the file/line. |
| Fit | 5 | One source/test site per row; long scopes remain complete in JSON and `--full`. |
| Craft | 4 | Rows fit 80 columns, paths and scopes remain unambiguous; the same grammar survives pipes. |
| Theme safety | 5 | Strict checks pass four palettes, C locale and NO_COLOR with no allowlist. |
| Machine contract | 5 | Sorted typed JSON contains two tracked sites; malformed markers fail, absent ids return an empty array. |

Scored by zealous-falcon-45 on November 3, 2026 after viewing the 80-column light, dark and Solarized sheets. This is producer evidence, not an independent verdict.

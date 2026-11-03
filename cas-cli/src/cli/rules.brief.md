# Rule reference navigation

| Field | Contract |
| --- | --- |
| First two lines | Give the number of references, then the command flag for complete machine output. |
| Scannable | One sorted path and line per comment, followed by its scope. |
| Readable | Scope stays JSON-quoted so embedded quotes and newlines remain unambiguous. |
| Machine output | `--json` emits one array of `{rule_id, path, line, scope}` objects, with repository-relative paths and one-based lines, no banner. |
| Omitted | No rule mutation, credential access, untracked/ignored-file scan, symlink traversal or guessed stakeholder attribution; complete values are available with `--json` or `--full`. |

The command works from any subdirectory of a Git checkout without Cassy login or initialization. It scans tracked working-tree text, including unstaged edits. An invalid marker for the requested rule fails with its path and line; unrelated rule ids do not affect the query.

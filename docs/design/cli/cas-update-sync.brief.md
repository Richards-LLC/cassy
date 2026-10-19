# `cas update --sync`

| Field | Contract |
| --- | --- |
| **First two lines** | The sync heading names the file group, and each changed file gets a success mark that remains readable in the active locale. |
| **Scannable** | Sync phases are grouped by harness, with one marked line per changed configuration or built-in sync result. |
| **Readable** | File paths and update counts explain what changed without adding terminal-only meaning to color or glyphs. |
| **Machine output** | `--json` emits one receipt object with config and built-in update counts, rules and skills totals, factory-tooling status, gitignore state, and `managed_files`; it contains no human status marks. |
| **Omitted** | Unchanged files stay omitted from human output; detailed managed-file reconciliation remains available through `--json`. |

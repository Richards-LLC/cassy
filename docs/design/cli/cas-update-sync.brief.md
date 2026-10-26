# `cas update --sync`

| Field | Contract |
| --- | --- |
| **First two lines** | The sync heading names the file group in bold terminal-default foreground; changed files get a localized success mark followed by default-foreground text. |
| **Scannable** | Sync phases are grouped by harness, with one marked line per changed configuration or built-in sync result. |
| **Readable** | File paths and update counts explain what changed; only the one-cell success mark uses semantic color, while headings and message text use the terminal's default foreground. |
| **Machine output** | `--json` emits one receipt with `config_updated`, `builtins_updated`, `builtin_reference_conflicts`, `codex_config_updated`, `codex_builtins_updated`, `codex_builtin_reference_conflicts`, `grok_builtins_updated`, `grok_builtin_reference_conflicts`, `opencode_builtins_updated`, `rules_synced`, `rules_removed`, `skills_synced`, `skills_removed`, `factory_tooling`, `builtin_gitignore_updated`, `builtin_gitignore_tracked`, and `managed_files`; no human status marks are written to stdout. |
| **Omitted** | Unchanged files stay omitted from human output; detailed managed-file reconciliation remains available through `--json`. |

## Contrast target

Keep body text in the terminal's default foreground. The one-cell `✓` success mark uses `#28914b`, which must maintain at least 3:1 on dark, light, Solarized-dark, and Solarized-light terminal backgrounds. The actual update command's 80/120-column palette captures, pipe, `NO_COLOR`, and JSON checks remain pending for the assembled v50 flow walk.

## Preserved reference warning (`--sync` and `--user`)

| Field | Contract |
| --- | --- |
| **First two lines** | A WARN names the preserved reference count and harness location before explaining why Cassy left local content intact. |
| **Scannable** | One warning block lists every preserved relative path, followed by the review and replacement remedy. |
| **Readable** | The existing explanation and advice wrap losslessly within 80 display cells, with continuation indentation; no ellipsis replaces prose or paths. Only the WARN mark may carry colour. |
| **Machine output** | Existing conflict arrays keep the original complete paths; human wrapping never enters the JSON document. |
| **Omitted** | Empty conflict results emit no warning; user directories name `cas update --user`, project directories name `cas update --sync`. |

Terminal palette captures and critique will use the supervisor's exact-tip candidate.

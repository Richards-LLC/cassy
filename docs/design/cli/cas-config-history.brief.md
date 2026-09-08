# Brief: `cas config describe history.github_repo`

| Field | Sentence |
| --- | --- |
| First two lines | The history repository setting is named immediately, followed by its source key and effective unset default. |
| Scannable | Fixed fields show the key, section, type, description, current value, default, and examples in that order. |
| Readable | The description explains that an unset override resolves from the checkout's GitHub origin and remains separate from `issues.repo`. |
| Machine output | `--json` returns one object with `key`, `name`, `section`, `description`, `type`, `default`, `current_value`, `modified`, `advanced`, `requires_feature`, `constraint`, and `examples`; nothing else appears on stdout. |
| Omitted | Resolver details and origin parsing stay in the history implementation and are not repeated in the config screen. |

## Critique

terminal-qa: PASS cas-config-history-4f2d-fixed · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-4f2d/terminal-qa/cas-config-history-fixed/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The setting name is the first line, and the key follows before the description and values. |
| Fit | 5 | The describe view uses fixed fields for scannable metadata and one short description. |
| Craft | 5 | The longest text line is 77 cells at both 80 and 120 columns; no row wraps. |
| Theme safety | 5 | The title uses bold default text; all four palettes, monochrome, and C-locale runs pass. |
| Machine contract | 5 | The JSON run emits one object with the documented stable fields and no banner. |

Scored by Codex on 2026-09-08; floor holds.

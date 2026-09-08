# Brief: `cas known-repos prune-missing`

| Field | Sentence |
| --- | --- |
| First two lines | The command reports how many gone registry roots are safe to remove, then names live roots without `.cas/` and their manual remedy. |
| Scannable | One count line states the dry-run or removal verdict; a second count line groups retained live roots without a Cassy store. |
| Readable | The retained-root row explains that the path is still present and offers one copyable `cas init` or `cas known-repos forget <path>` remedy. |
| Machine output | This subcommand has no `--json` mode; stdout is stable human text and repository files are never changed by either dry-run or apply. |
| Omitted | Individual retained paths are omitted from the aggregate receipt; use `cas known-repos list` to inspect the registry before choosing `forget`. |

## Critique

terminal-qa: PASS cas-2878-known-repos · 11 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-2878/terminal-qa/known-repos/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The dry-run/removal count is first, followed immediately by the retained-root state and remedy. |
| Fit | 5 | Aggregate counts collapse repeated states without printing one paragraph per registry row. |
| Craft | 4 | The finding and remedy are separate lines and fit the 80-column capture. |
| Theme safety | 5 | Dark, light, Solarized, `NO_COLOR`, and C-locale runs pass without unsafe colour or glyphs. |
| Machine contract | 4 | The command has no JSON contract; piped output remains plain, append-only text. |

Scored by the worker on 2026-09-08; hierarchy, fit, and craft floors hold.

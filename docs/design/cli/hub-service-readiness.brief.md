# `cas hub service status` launch-readiness brief

- **First two lines:** Line one reports service state and the launch issue count; line two gives the first copyable repair command.
- **Scannable:** Ready CLIs use one short row; unavailable profiles use a summary, a home-relative path, and one repair command.
- **Readable:** Profile paths use `~` when they are under the home directory; repair commands name the affected CLI and profile.
- **Machine output:** `--json` remains one `ServiceReport` object with `launch_readiness[]`; the existing `cli`, `profile`, `ready`, and `detail` fields stay stable, and each `detail` keeps complete error text and absolute paths.
- **Omitted:** Ready executable paths and repeated error prose are omitted from the human scan and remain available in JSON.

## Critique

`terminal-qa: PASS cas-hub-service-status · 12 runs · 0 fail · 0 warn · 0 allowed`

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | The first line reports “not installed” and “3 launch issues”; line two is `Fix: cas claude login main`. |
| Fit | 4 | Each unavailable CLI has one short cause; repair commands are grouped at the top or beside the row. |
| Craft | 4 | The 80- and 120-column runs top out at 71 cells; each finding and remedy stays on one line. |
| Theme safety | 5 | Dark, light, both Solarized palettes, `NO_COLOR`, and `LC_ALL=C` pass. |
| Machine contract | 5 | The JSON run emits one document with full profile paths and detail text. |

## Clean-home account resolution

An absent implicit main profile keeps its home-relative path and copyable
`cas claude login main` / `cas codex login main` remedy. Explicit unknown
selectors keep the registry refusal and login command; readiness never creates
an account folder. The original first-two-lines and JSON contracts above apply.

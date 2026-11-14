# Brief: hub command help

| Field | Sentence |
| --- | --- |
| First two lines | Preserve the hub purpose and standard Clap usage before the command and option lists. |
| Scannable | Keep command names and option spellings in their existing columns, wrapping descriptions at 80 columns. |
| Readable | Preserve all descriptions, defaults and aliases while wrapping between words. |
| Machine output | Help exits before dispatch and remains text, including when `--json` is present; command JSON schemas stay unchanged. |
| Omitted | No commands, descriptions or defaults are removed to make the help fit. |

## Rendering decisions

Cap the hub command's automatic help width at 80 columns so terminals and pipes
receive readable output. Keep Clap's existing ordering and presentation.
The same help remains readable at 120 columns without adding content.

design: DESIGN.md@b43940e43

## Critique

terminal-qa: PASS cas-hub-help · 11 runs · 0 fail · 0 warn · 0 allowed · `<artifacts>/cas-2754/terminal-after/report.json`

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Existing purpose, usage, command and option groups keep their ordering. |
| Fit | 5 | Rendered pipe output has 38 lines, maximum width 80 and zero overlong lines; before had 34 lines, maximum 97 and six overlong lines. |
| Craft | 4 | Wrapping preserves every whitespace-normalized word, default and alias; the terminal gate finds no split tokens. |
| Theme safety | 5 | 80/120-column dark, light and Solarized captures, plus NO_COLOR and C-locale runs pass. |
| Machine contract | 5 | Real CLI short and long help exit successfully; command dispatch and JSON handlers are untouched. |

Scored by the worker on 2026-11-10. Captures use build
`16b52fb9ac756c6e5fccbf3a13c4b6660421d1b3`; this critique-only edit changes
no evaluated CLI input. The final commit receives its own capped test receipt.

The generic terminal gate gives piped output no width constraint. The real CLI
regression and explicit line measurement cover the 80-column pipe requirement.

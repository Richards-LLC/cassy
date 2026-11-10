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

Pending rendered terminal proof at the delivered build.

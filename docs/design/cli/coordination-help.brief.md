# Brief: `cas coordination <command> --help`

| Field | Sentence |
| --- | --- |
| First two lines | Name the selected thread-ownership operation before showing how to invoke it. |
| Scannable | Show the required flags in the usage synopsis, then each option with its explanation on the next line. |
| Readable | Explain where IDs come from, generation checks, retry identity, and the transfer offer lifetime. |
| Machine output | Help remains clap text with exit 0, including with `--json`; actual coordination operations retain their one JSON receipt document. |
| Omitted | No Cloud state, credentials, thread content or network activity is needed to read help. |

## Rendering decisions

- Scope help layout to coordination, preserving other commands' help and all parsing.
- Keep every required flag visible in the synopsis. For the longer claim mutations,
  use multiline clap help templates with whole flag/value pairs on their own lines.
- Put descriptions below options so inherited flags and long value names fit 80
  columns without dropping their explanation. Keep the same content at 120 columns.
- Describe an optional request ID as retry identity, a required status request ID as
  the original request to reconcile, and generation as the current receipt value.
- Read-only help cells on the actual assembled candidate belong to the supervisor's
  single flow walk: all seven subcommands at 80/120, piped, NO_COLOR and LC_ALL=C.

## Critique

Baseline installed `cas 3.64.0 (d17444d 2026-10-19)` reproduces blank flag
descriptions and a 146-cell transfer usage line. Its terminal gate fails at both
80 and 120 columns; the inherited `--full` row is also 86 cells at 80 columns.

After-candidate gate and visual critique: NOT EXERCISED by this worker;
supervisor-owned assembled flow walk, per assignment #4200337. This brief makes
no passing real-build terminal claim until those captures are reviewed.

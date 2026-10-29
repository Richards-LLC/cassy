# Ledger check and scaffold

Design: `DESIGN.md@b43940e43`, inheriting `hub-web/DESIGN.md`.

| Field | Decision |
| --- | --- |
| First two lines | `cas qa ledger check` reports valid row count or diagnostic count, then names the repair command; `init` reports the written skeleton and the command to check it. |
| Scannable | Findings appear in source order, with one line for ledger line number, row ID and field, followed by the cause and remedy; successful checks collapse to one count line. |
| Readable | Each finding explains the evidence rule violated and the smallest repair, while making no claim that a capture proves its expected outcome. |
| Machine output | Global `--json` emits one document with `ok`, `path`, `rows`, `counts` and `diagnostics` (`line`, `row_id`, `field`, `code`, `message`, `remedy`); init adds the task identity and created path, with no banner or ANSI on stdout. |
| Omitted | The row grammar is owned by the shared executable parser and generated scaffold, while human capture judgment, provenance and task-derived proof sufficiency remain in the verifier and existing contextual gates. |

`check <path>` is read-only and exits unsuccessfully for invalid input. `init
--task <id>` uses the receiving project's task artifact namespace, includes
predicted journey rows, uses honest NOT A ROUTE defaults for no-code delivery,
and preserves existing evidence by refusing to overwrite its destination.
Unexecuted live journey rows remain NOT EXERCISED rather than fabricated PASS.

The renderer must fit 80 columns, keep long paths out of finding columns, and
preserve line/row identity in the JSON document. Its diagnostics and close's
pre-dispatch refusal consume the same report. Human and JSON behavior, pipe,
NO_COLOR and locale captures require the actual candidate before critique.

## Critique

Pending implementation and candidate terminal-qa execution. No output or
mechanical PASS is claimed by this concept brief.

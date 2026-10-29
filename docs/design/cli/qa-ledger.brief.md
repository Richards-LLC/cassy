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

Candidate: `cas 3.87.0 (6709dce 2026-10-29)`, supervisor-built at
`6709dced8d73cb24f7f4b5cd4c3b6ad8d33c259b`, SHA-256
`81e20c004910fc5fabf9206c9cdcef0f4282bea0a32921bb120cc06dee3fedc2`.

Receipt prefixes (complete reports and paths live in the task evidence bundle):

- `terminal-qa: PASS qa-ledger-valid · 12 runs · 0 fail · 0 warn · 0 allowed`
- `terminal-qa: PASS qa-ledger-invalid · 12 runs · 0 fail · 0 warn · 0 allowed`
- `terminal-qa: PASS qa-ledger-init · 12 runs · 0 fail · 0 warn · 0 allowed`

Each covers 80/120 columns on four palettes, pipe, NO_COLOR, C locale and JSON.
The expected invalid-ledger exit is 1; its JSON stdout remains one document.
An initial init capture harness incorrectly assumed that the generated artifact
path belonged under the fixture repository. It belongs under the isolated
fixture HOME; correcting that ownership assertion produced the init PASS.
The original failed harness captures remain in the evidence bundle.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Line one names valid/invalid and its count; line two prints the repair command. |
| Fit | 5 | Valid output collapses to one line; invalid output expands only the named line, row and field. |
| Craft | 4 | Hanging diagnostic indents fit 80 columns; init offers JSON for its full path. |
| Theme safety | 5 | All four palette captures, NO_COLOR and C locale passed with no allowances. |
| Machine contract | 5 | Valid and invalid checks plus init emit one JSON document; failure retains exit 1. |

Read the 80-column light/dark HTML and pipe captures, then inspected the
[four-palette render](qa-ledger/palettes.png). The browser MCP launcher could
not start its host sandbox; a disposable headless Chrome profile rendered the
same local report successfully. [Valid](qa-ledger/valid.txt) and
[invalid](qa-ledger/invalid.txt) captures are committed with this critique.

The real candidate walk also checked predicted FLOW-J1 scaffolding, the
no-code route default, check/init round trip, refusal to overwrite evidence,
and row/line diagnostics for an unowned FAIL. The final critique/capture
commit changes only these documentation files; evaluated production, builtin
and test inputs stay identical to the candidate revision.

# Single-consumer owner groups

| Field | Contract |
| --- | --- |
| First two lines | Journey check keeps its problem/debt verdict first; a single-consumer group adds debt without changing success exit status. |
| Scannable | One warning line per single-consumer group names the group and its only journey within 80 columns. |
| Readable | The warning prompts review of shared ownership membership; repeated mentions in one journey count as one consumer, while unused and shared groups stay quiet. |
| Machine output | The existing JSON document adds single_reference_owner_groups mapping complete group names to their one journey ID, plus the matching debt item and kind count. |
| Omitted | No inferred memberships or selection changes; long human IDs elide with a JSON escape, and JSON retains complete names. |

## Critique

`terminal-qa: PASS cas-ad1e-short · 12 runs · 0 fail · 0 warn · 0 allowed`

`terminal-qa: PASS cas-ad1e-long · 12 runs · 0 fail · 0 warn · 0 allowed`

Supervisor receipt #543594 binds candidate
`7426cc58c30b9bf93ea03143c5ad68e6f49e1955`,
`cas 3.72.0 (7426cc5 2026-10-22)`, to both native terminal matrices.
Short and long audit runs exit zero and prove full IDs in JSON, one debt item,
no problems, and no warnings for shared or unused groups. Repeated mentions
inside one journey still count as one consumer. The gate uses
`--json-flag --json --escape-flag --json`; captures are task cas-ad1e
`terminal-qa/short/` and `terminal-qa/long/`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | DEBT leads with zero problems and one debt item; the named warning follows the kind/history summary. |
| Fit | 5 | One compact warning fits80 for both ID sizes; complete names remain in JSON. |
| Craft | 4 | Short IDs read in full; long IDs elide with one inline full-JSON hint. |
| Theme safety | 5 | Both surfaces pass four palettes, NO_COLOR, pipe and C locale with ASCII elision. |
| Machine contract | 5 | Existing arrays and summary remain; the additive group-to-journey map agrees with non-blocking debt. |

Scored by young-otter-20 on 2026-10-22 against supervisor captures and audits.
The warning makes missing membership visible without adding memberships or
changing path selection. Zero and multiple consumers produce no new debt.

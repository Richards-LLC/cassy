# Journey bootstrap and status output

| Field | Contract |
| --- | --- |
| First two lines | State READY, DEBT, MISSING or INVALID with the journey count, then give the next command. |
| Scannable | One count row names seeded entries and unowned surface files; the registry path is last. |
| Readable | Missing input gives `cas qa journeys --seed`; existing input gives `cas qa journeys --check`. |
| Machine output | Global `--json` emits one object: path, present, valid, journeys, seeded, debt_count, unowned_surfaces, problems, added. |
| Omitted | Route guesses, full journeys and product prose remain in YAML, which the operator curates. |

Plain ASCII output uses no color or cursor controls. JSON retains complete paths.
The doctor row shares these counts and gives the seed command when no registry exists.

## Critique

Candidate binary terminal captures belong to epic assembly; NOT VERIFIED here.
The two CLI fixtures exercise seed, merge, check and JSON status dispatch.

# Release-train status and receipt brief

| Field | Contract |
| --- | --- |
| **First two lines** | The status output identifies the run and whether publication is verified; the next evidence line exposes green-to-published latency with intervention count. |
| **Scannable** | One stable `KEY=value` receipt row carries interventions, blocker stage names, and both hand-off delays; the human status keeps those metrics beside the latency row. |
| **Readable** | Operators can read the UTC intervention log to identify the subcommand, caller session, source kind, and canonical stage for each detour. |
| **Machine output** | `release-latency-receipt.sh` emits `INTERVENTIONS`, `BLOCKERS`, `GREEN_TO_PIPELINE_SECS`, and `MERGED_TO_PUBLISHER_SECS` as stable `KEY=value` fields alongside the existing latency fields. |
| **Omitted** | Individual intervention log lines stay out of the normal status summary; inspect `<run dir>/interventions.log` when the caller, timestamp, or subcommand is needed. |

## Critique

terminal-qa: PASS release-train-status · 11 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-09eb/terminal-qa/report.json
| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Publication state and green-to-published evidence precede the intervention and hand-off rows. |
| Fit | 5 | Piped, 80-column, 120-column, C-locale, and no-color captures have zero overflow. |
| Craft | 4 | Metrics use stable uppercase receipt keys with one field group per line. |
| Theme safety | 5 | All four palettes, no-color, and C-locale runs pass. |
| Machine contract | 5 | Receipt fields are newline-delimited `KEY=value` records with no extra output. |

Scored by Codex on 2026-09-18; floor holds.

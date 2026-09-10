# Brief: `cas history embed`

| Field | Sentence |
| --- | --- |
| First two lines | The first line states whether embedding completed and how many units remain; the next line names any truncation or the single actionable provider boundary. |
| Scannable | The default output uses one summary line, one truncation receipt when applicable, and one compact quarantine row per refused unit. |
| Readable | A truncated unit is explicitly identified as a successful searchable outcome, while a true quarantine keeps the provider cause and one copyable retry command. |
| Machine output | `--json` emits one object with `capability_absent`, `embedded`, `skipped`, `requests`, `pending_after`, `quarantined_this_run`, `quarantined_total`, `truncated_this_run`, `truncated`, `requeued`, and `problems`; no human lines appear. |
| Omitted | Individual request payloads, token estimates, and provider response bodies remain out of the normal success path; only quarantine details are printed because they require action. |

## Rendering decisions

- Truncation is a receipt, not a warning: it means the unit received a searchable vector under the provider token budget.
- The JSON boolean `truncated` is paired with the numeric `truncated_this_run` count so scripts can branch on presence while operators retain scale.
- Quarantine output remains separate from pending work and keeps the existing `cas history embed --retry-quarantined` escape hatch for non-size refusals.

## Critique

terminal-qa: PASS cas-history-embed-cas-1734 · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-1734/terminal-qa/cas-history-embed-cas-1734/report.json

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | The human summary is the only line in the empty-queue case; truncation is emitted immediately after the summary when present. |
| Fit | 5 | Human output stays compact, while `--json` remains one typed document with both boolean and numeric truncation fields. |
| Craft | 4 | The new receipt uses one unit/count grammar and terminal QA reports a 72-cell maximum human line at 80 and 120 columns. |
| Theme safety | 5 | All four palettes, piped, `NO_COLOR`, and C-locale captures pass with no findings. |
| Machine contract | 5 | The JSON capture is one document, exit 0, and includes `truncated` plus `truncated_this_run` without human text. |

Scored by Codex on 2026-09-10; hierarchy, fit, and craft floors hold.

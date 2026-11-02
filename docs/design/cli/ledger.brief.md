# Release ledger evidence scope

design: DESIGN.md@b43940e43

| Field | Contract |
| --- | --- |
| First two lines | The existing release heading and time window identify which release the numbers describe. |
| Scannable | Turn totals precede interventions; channel rows show wakes/no-ops and measured tokens/minutes with missing-data counts. Feature rows show session-attributed measurements and supervisor verdicts; one count identifies actual excluded measurements with unknown scope; an empty report says none recorded when the excluded count is zero. |
| Readable | The exclusion explains that historical measurements may contain lifetime totals, so readers cannot mistake them for release evidence. |
| Machine output | `--json` emits one report object, preserving `features` and adding `turns`, `turns_by_channel` (wakes, no_op_wakes, input_tokens, output_tokens, minutes, missing_tokens, missing_minutes) and `excluded_unscoped_measurements` independently of its measured counts and verdicts. |
| Omitted | Excluded historical payloads remain in the append-only store; their totals and measured verdicts do not enter release feedback. |

Was: Backfill dated lifetime retrieval totals as new measurements, and reports
added those snapshots to the release window. → Now: Backfill records dated task
and verification history; session end records scoped feature evidence. Reports
exclude historical actorless measurements and disclose their count.

This correction preserves the report's existing section order and plain-text
format. It does not redesign the release heading or measurement policy.


Turn records cover Claude, Codex and Grok. The report reads only immutable quality
rows; it does not open transcripts. Unknown tokens/minutes are counted as missing
instead of being described as measured zero. Raw prompts, commands and replies
remain outside the report and Cloud payload.

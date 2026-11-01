# Release ledger evidence scope

design: DESIGN.md@b43940e43

| Field | Contract |
| --- | --- |
| First two lines | The existing release heading and time window identify which release the numbers describe. |
| Scannable | Feature rows show session-attributed measurements and supervisor verdicts; one count identifies excluded measurements with unknown scope. |
| Readable | The exclusion explains that historical measurements may contain lifetime totals, so readers cannot mistake them for release evidence. |
| Machine output | `--json` emits one report object, preserving `features` and adding `excluded_unscoped_measurements` independently of its measured counts and verdicts. |
| Omitted | Excluded historical payloads remain in the append-only store; their totals and measured verdicts do not enter release feedback. |

Was: Backfill dated lifetime retrieval totals as new measurements, and reports
added those snapshots to the release window. → Now: Backfill records dated task
and verification history; session end records scoped feature evidence. Reports
exclude historical actorless measurements and disclose their count.

This correction preserves the report's existing section order and plain-text
format. It does not redesign the release heading or measurement policy.

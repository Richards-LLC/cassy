# Journey check history recovery

| Field | Contract |
| --- | --- |
| First two lines | Keep the journey verdict and problem/debt counts first; a damaged comparison receipt must not obscure the current check. |
| Scannable | Retain debt-kind counts and change summary, then one history-reset notice when the prior receipt is replaced. |
| Readable | The notice states that history restarted at the current check; verbose mode already shows individual findings and needs no hint to itself. |
| Machine output | `--json` remains one CheckReport document; debt_summary adds optional history_notice, retains history_status=recorded, and uses null previous_total/delta/grew for the fresh baseline. |
| Omitted | Do not dump corrupt metadata or parse details; genuine database/write failures retain history_error and unavailable status. |

The current metadata key alone is replaced under the existing transaction; other
project/registry histories stay intact. No manual deletion is required.

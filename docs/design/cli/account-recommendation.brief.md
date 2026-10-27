# Account recommendation

| Field | Decision |
| --- | --- |
| First two lines | Ask which Claude or Codex account to use; the cursor and recommended mark explain the proposed choice before Enter. |
| Scannable | Preserve every account row and the new-login row, with one recommended mark beside the eligible subscription. |
| Readable | The recommendation gives the weekly reset interval and remaining percentage; excluded rows say why automatic selection skipped them. |
| Machine output | No new picker JSON mode; factory spawn receipts include the chosen directory and reason, and Commander profiles add recommendation or exclusion text. |
| Omitted | No automatic interactive launch or hidden account; full quota details stay in `cas accounts list`. |

Design: DESIGN.md, inheriting hub-web/DESIGN.md. Existing terminal colours and selection treatment are retained.

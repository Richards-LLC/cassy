# Project hold

| Field | Contract |
| --- | --- |
| First two lines | State whether the project is held, then give the operator's lift command or confirm normal dispatch and merge admission. |
| Scannable | Reason, operator and recorded time appear as separate labeled rows. |
| Readable | The operator's reason explains why the project stopped; status exposes the complete audit with `--json`. |
| Machine output | `cas --json factory hold status` returns one document with `active` (reason, operator, set_at) and `history` (action, reason, operator, at). |
| Omitted | Worker and delivery inventories stay off the hold screen; normal task queries resume when the operator lifts it. |

Text uses plain ASCII and no ANSI styling; long human values use a bounded preview with `--json` as the full read.

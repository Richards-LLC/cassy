# Cloud memory fold conflict warning

| Field | Contract |
| --- | --- |
| First two lines | A completed pull with a folded-memory conflict warns that both copies were retained, then names `cas cloud conflicts --json` to inspect them. |
| Scannable | One count collapses all folded-memory conflicts; sync status and doctor retain the warning after the pull advances. |
| Readable | The journal names the occupied ID and retains the incoming and local before-images; ordinary sync output shows only the count and remedy. |
| Machine output | Sync/pull/status JSON adds `memory_fold_conflicts`, an array of opaque IDs; `cas cloud conflicts --json` exposes both before-images in `discarded_row_json`. |
| Omitted | Memory content and full ID lists remain in the conflict journal, preserving compact output and avoiding unexpected content disclosure. |

Existing clean-pull, status and doctor output stays unchanged when the conflict set is empty.

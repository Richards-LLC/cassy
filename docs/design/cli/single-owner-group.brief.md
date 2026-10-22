# Single-consumer owner groups

| Field | Contract |
| --- | --- |
| First two lines | Journey check keeps its problem/debt verdict first; a single-consumer group adds debt without changing success exit status. |
| Scannable | One warning line per single-consumer group names the group and its only journey within 80 columns. |
| Readable | The warning prompts review of shared ownership membership; repeated mentions in one journey count as one consumer, while unused and shared groups stay quiet. |
| Machine output | The existing JSON document adds single_reference_owner_groups mapping complete group names to their one journey ID, plus the matching debt item and kind count. |
| Omitted | No inferred memberships or selection changes; long human IDs elide with a JSON escape, and JSON retains complete names. |

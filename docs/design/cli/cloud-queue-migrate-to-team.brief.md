# Legacy personal queue recovery

| Field | Contract |
| --- | --- |
| First two lines | Preview reports the count of rows eligible for team enqueue and the explicit apply command; it never calls an enqueue a delivery. |
| Scannable | Queue identifiers, entity types and fixed decision reasons follow the verdict in queue order. |
| Readable | One remedy sets team_only only after the operator declares the project team-only, then previews the backfill before applying. |
| Machine output | Migration JSON is one document with dry_run, team_id, project_id, enqueued and every row's id/entity_type/entity_id/eligible/reason; sync JSON includes held personal counts and the recovery command. |
| Omitted | Queued content, diagnostic payloads, credentials and conflict history snapshots never appear in migration output; --json carries full identifiers if human rows abbreviate them. |

No assembled worker binary build is authorized. Terminal captures on a candidate binary remain an assembly cell; renderer checks use the real output helper in named tests.

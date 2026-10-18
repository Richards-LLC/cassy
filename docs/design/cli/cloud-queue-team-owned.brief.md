# Team-owned queue cleanup

- **Reader:** An operator repairing a mixed personal queue in a team-linked project.
- **Decision:** Preview exact rejected IDs and retain every mutation without an equivalent, acknowledged team write.
- **Hierarchy:** Start with eligible/retained counts, then queue ID and eligibility reason; provide one scoped cleanup command.
- **Layout:** Plain ASCII lines fit 80 columns; neither preview nor errors display queued payloads. Large selections use JSON.
- **Machine output:** `--json` returns one document with `dry_run`, `purged`, `rows` (`id`, `entity_type`, `entity_id`, `eligible`, `reason`), personal scope and `team_owned_project` reason. `--ids` selects exact queue IDs; an unscoped mutation is refused.

## Receipt authority

Ownership rejection proves policy only. Cleanup requires a version-1 `team_delivery_ack:<queue-id>` record in existing `sync_metadata`, written transactionally before consuming the acknowledged team queue snapshot. It binds the personal row creation time, entity, operation, team, canonical project and SHA-256 of the complete serialized transmitted JSON. It contains no private payload. Per-row `inserted` or `updated` is required; aggregate counts, `skipped_lww`, deletes, transformed payloads and unknown response shapes do not establish equality. Cleanup rechecks the receipt and personal row in one transaction and removes both together. A changed or legacy row remains queued.

## Critique

Pending exact-build terminal capture and independent review.

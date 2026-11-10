### Fixed — Cloud sync

- **Recover retained dependency failures** — Queue details expose item IDs, scope and repair advice; retry selected items with `--retry --ids`. Scoped pulls revalidate retained dependency endpoints after ownership repairs, while preserving foreign boundaries and local graph data. Replayed identical conflict snapshots no longer grow the journal.

### Fixed — Merge publication

- **Delivery merges retain unrelated unpublished target work** — Merging a
  delivery refuses an ahead local target before publishing its existing
  commits. The refusal lists local-only commit identities and the fetched
  target; source and target work remain preserved. Equal and behind targets
  retain their ordinary merge path, and transactional publication shares the
  fresh-target fence.

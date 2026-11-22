### Fixed — Merge publication

- **Delivery merges retain unrelated unpublished target work** — Merging a
  delivery refuses unrelated unpublished target history before publishing its
  commits. The refusal lists local-only commit identities and the fetched
  target; source and target work remain preserved. Equal and behind targets
  retain their ordinary merge path, and transactional publication shares the
  fresh-target fence. The delivery's own commits and integration merge remain
  eligible; typed receipt failures and detached review validation run before
  publication checks.

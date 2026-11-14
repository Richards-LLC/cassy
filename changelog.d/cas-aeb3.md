### Fixed — Release closeout

- **Reverted deliveries stay open** — Release inventory and closeout verify the task's own content at the release tag, excluding train imports and recognizing metadata folded into release documents. Reverted changes and bound review rows marked as reverted remain held for re-delivery instead of being closed as shipped.
- **Queue landing preserves release proof** — Public content checks measure the whole task-owned delivery at the release cut, so a later loss cannot hide behind an intact final commit while genuinely landed deliveries still close. Reconciliation retains delivery-cycle identity alongside the scoped content and reverted-review checks.

### Fixed — Release closeout

- **Reverted deliveries stay open** — Release inventory and closeout verify the task's own content at the release tag, excluding train imports and recognizing metadata folded into release documents. Reverted changes and bound review rows marked as reverted remain held for re-delivery instead of being closed as shipped.

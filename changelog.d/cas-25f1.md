### Fixed — Factory

- **Publish new release trains at once** — A release train opened on task create now has its branch on origin immediately, so workers retargeted there can fetch it without a hand push. A failed push is reported on the create receipt.

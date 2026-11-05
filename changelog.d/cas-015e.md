### Fixed — Factory

- **Reviews follow the published target** — Candidate review now uses the fetched target on origin even when the local branch has an unpublished merge. Rolling that local merge back no longer triggers another verifier when the published target advances without changing the reviewed delivery. Source, plan, evidence and delivery-content checks still apply; merge-queue CI checks interactions with other deliveries.

### Fixed — Verification

- **Read-only investigations stay verified while the epic moves** — A task closed with `no_delivery=true` (an investigation with no code) now pins its verification to the exact commit it was reviewed at, as no-code tasks already did. A sibling merge into the epic, the worker starting its next task, or uncommitted notes in the checkout no longer invalidate the verifier's dispatch and force a second review.

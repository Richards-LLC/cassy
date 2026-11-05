### Fixed — Verification

- **Read-only investigations stay verified while the epic moves** — A close that Cassy measures as delivering nothing, from a clean checkout, now pins its verification to the exact commit it was reviewed at, whether or not it declared `no_delivery`. This applies to factory tasks with a recorded lane; legacy tasks keep the strict proof. A sibling merge into the epic, the worker starting its next task, or uncommitted notes in the checkout no longer invalidate the verifier's dispatch and force a second review.

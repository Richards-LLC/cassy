### Fixed — Verification

- **Read-only investigations stay verified while the epic moves** — A close with no commit of its own (an investigation, whether or not it declared `no_delivery` or `no-code`) now pins its verification to the exact commit it was reviewed at. A sibling merge into the epic, the worker starting its next task, or uncommitted notes in the checkout no longer invalidate the verifier's dispatch and force a second review.

### Fixed — Factory

- **Close reviewed deliveries after release progress** — Accept a synthetic review candidate when its exact merge tree landed under a different commit. Batch receipts retry close for deliveries with their own review, preserving landed approvals after later sibling edits while retaining unlanded-delivery and dedicated-review refusals.

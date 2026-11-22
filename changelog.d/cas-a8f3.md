### Fixed — Releases

- **Recognize lane CI that finishes after a docs merge** — Release cuts now accept successful checks at an Advisory delivery's exact anchor, including Docs Lint with classified code-job skips. Pending, failed, missing, or different-anchor checks still hold the delivery; reopening and merging it again is unnecessary.

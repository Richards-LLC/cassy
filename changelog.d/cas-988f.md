### Fixed — Delivery content proof

- **Content proof without a work window no longer claims unnamed commits** — Was:
  after the same-tree merge recovery change, a content check run with no work
  window recovered snapshot ranges under an epoch creation floor, so an
  unnamed commit behind a clean sync merge counted as task delivery. Now: the
  snapshot recovery runs only with a real work window; without one the
  ordinary task range is used, as before.

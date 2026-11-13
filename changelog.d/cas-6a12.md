### Fixed — Factory recovery

- **Accurate local-only retained-work warnings** — Clean checkouts already at their local task target no longer appear as wholly unpushed history when no remote exists. Dirty or ahead worker work stays visible; push recovery names a configured remote and is omitted when none exists.

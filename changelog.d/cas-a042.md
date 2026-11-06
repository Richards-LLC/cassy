### Fixed — Session context

- **Keep handoffs on their owning machine** — Saving a handoff no longer retires a peer machine's current note. Session start selects the local predecessor, including legacy notes with retained author provenance, and shows peer notes only as labeled pointers. Worker startup omits role-wide handoff bodies from unrelated work.

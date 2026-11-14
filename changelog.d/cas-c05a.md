### Fixed — Factory

- **Conflicts caught before merge admission** — Parking refuses deliveries that conflict with the current train and names the paths. When the train advances, conflicting parked siblings return to their workers with their existing branch and ownership.
- **Conflict admission policy change** — Conflicting deliveries now stay with their worker when parking is refused, with an explicit merge alternative. This retires the former park-then-flag recovery contract. Historical batch reconciliation failures retain their existing delivery state and are not treated as new merge candidates.

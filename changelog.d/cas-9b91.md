### Fixed — Tasks

- **Explicit supervisor priority sticks on findings** — Was: finding labels silently overrode a supervisor's priority, and the update reply claimed priority and labels changed. Now: an explicit priority wins (GH #1636) and the reply lists only fields that changed.

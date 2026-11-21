### Fixed — Epic close

- **Green CI survives intentional post-merge skips** — Epic close can reuse fully green required checks from the exact merge source when its entire tree matches the landed merge. Skipped jobs must match successful source checks by name and app, and missing required-check metadata, changed trees or unsuccessful checks still refuse close. Required contexts also support project-specific CI job names.

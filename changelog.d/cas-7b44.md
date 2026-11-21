### Fixed — Deliverables

- **CSS-mapped brands carry every report role the close gate requires** — Was:
  a DESIGN.md whose CSS `maps` named a muted-text role but no evidence role
  branded and finalized a customer report, then the report close refused it
  for a missing `--evidence`. Now: the CSS-map brand path fills each missing
  report role from the project's own semantic alias, exactly as the
  design-spec value-key path does, so brand, finalize and the close gate
  agree. Roles the DESIGN.md maps explicitly still win.

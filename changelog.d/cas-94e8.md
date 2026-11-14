### Fixed — Hooks

- **H-11 allows command help** — Was: `cas config grant-write --help` was
  blocked as a policy mutation. Now: help and version output are recognized as
  non-mutating by the CLI parser before H-11 evaluates the command.

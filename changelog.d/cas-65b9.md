### Fixed — Factory

- **The user-level worker limit applies to every project** — Was: a project
  without its own `factory.max_workers` ignored `~/.cas/config.toml`, so
  `cas config get`, `list` and `reset` showed 8 and spawns were refused at
  9/8. Now: the limit resolves project, then user, then 8 in config readback,
  reset and spawn admission, and each names where the value came from.

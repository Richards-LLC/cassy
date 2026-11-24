### Fixed — Factory

- **Codex lanes spawn when workers route through the hub gateway** — Was: with
  Codex workers on the home gateway, `spawn_workers lane=standard` refused every
  Codex recipe because it looked for a local `~/.codex/auth.json` that no longer
  exists. Now: a lane checks each Codex recipe's model against the hub's model
  listing when the worker would route through the hub, and still uses the
  account's own catalogue when it would start direct.

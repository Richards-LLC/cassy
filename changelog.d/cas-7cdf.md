### Fixed — Factory

- **A routed build keeps the requested worker recipe** — Was: a `spawn_workers`
  routed to another machine dropped `cli`, `model`, `effort` and `lane`, so the
  receiving supervisor picked its own (Opus/high instead of Sonnet/high). Now:
  the route brief and the `BUILD_ROUTE` note carry the recipe; the receiving
  spawn fills any omitted field from it and refuses a conflicting one. A route
  with no recipe behaves as before.

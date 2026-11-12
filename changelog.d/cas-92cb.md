### Fixed — Release workflow

- **One full proof on the release tree** — Train pushes and cuts stop waiting for a duplicate full-suite receipt. The merge queue keeps its complete checks and flake handling, builds the CLI without a preceding check, and restores dependencies from a matching main-scope cache.

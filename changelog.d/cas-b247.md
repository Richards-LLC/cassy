### Fixed — Skills

- **`cas update` and `cas doctor` agree on built-in skill directories** — Was: a legacy database skill such as `playwright-debug` was written over the built-in `cas-playwright-debug`, so doctor kept reporting drift that `cas update` recreated. Now: the built-in owns its directory, and the sync names the skipped database skill so it can be archived.

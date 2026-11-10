### Fixed — Factory

- **Read-only SSH diagnosis** — Python checkout discovery using the default `glob.glob` behavior no longer falsely triggers the operator credential guard. Direct credential paths, explicit hidden-directory globs and requests to include hidden files remain refused.

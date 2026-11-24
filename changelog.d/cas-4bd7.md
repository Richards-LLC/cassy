### Fixed — Worker sandbox

- **Workers can no longer write to `~/Library/LaunchAgents`** — Was: listing that directory in `write_roots` made it writable inside the macOS worker sandbox, so a file planted there would run at the next login outside the sandbox. Now: the profile always denies it, whatever `write_roots` contains.

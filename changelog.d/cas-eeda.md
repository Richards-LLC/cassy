### Fixed — Factory

- **Background close outcomes survive lost continuations** — Late closes keep backend retirement waiting until their outcome is saved. Cancelled continuations record a failure and alert the supervisor; surviving daemons recover pending outcomes after a backend exits.

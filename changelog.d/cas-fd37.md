### Fixed — Factory worker hosts on macOS

- **A worker host on macOS stays attached to its own project** — Was: when a session's recorded project path was a symlinked form of the store root, such as `/var/...` for `/private/var/...` on macOS, worker-host coordination refused every send with "Worker host belongs to another project". Now: both paths are resolved to their physical location before comparison, as the other session checks already do.

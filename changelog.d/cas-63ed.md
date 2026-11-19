### Fixed — Factory

- **Recycle waits for its own build** — Worker recycling stays queued while that worker's verified detached build holds its target lease, then continues automatically when it releases. Unknown, completed and foreign-session holders still report a failure.

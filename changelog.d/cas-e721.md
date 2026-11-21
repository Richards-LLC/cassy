### Fixed — Task delivery targets

- **Explicit epic-child targets stay authoritative** — Creating a child with an explicit target branch now keeps that branch through task display, start and merge, including when it matches the epic base. Supervisor scope correction can repair an inherited effective target without changing the stored branch name. Omitted branch defaults continue to follow epic lane renames.

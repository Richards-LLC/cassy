### Fixed — Cloud sync

- **Skip busy stores before cloud setup** — Background sync checks the store lease before team discovery and registration, so an already syncing store makes no competing cloud requests. Entity sync still releases its lease before optional knowledge and quality work.

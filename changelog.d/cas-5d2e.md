### Fixed — Factory

- **Verifier launch names a busy store instead of a missing supervisor** — When the owner's authority lookup times out on a busy store connection, a factory verifier launch reports `verifier_factory_store_busy` with the real cause and the retry command, rather than "live registered supervisor required" for a supervisor that is live.

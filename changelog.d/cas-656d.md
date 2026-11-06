### Fixed — Factory

- **Release failed and obsolete build routes** — A peer build ask expires after three failed send attempts or a permanent refusal, restoring local admission. Pending asks are cancelled before retry when routing is disabled or the task is staffed locally, including through a batch spawn.

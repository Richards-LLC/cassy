### Fixed — QA evidence

- **Ledger scaffolds name the worker's build** — `cas qa ledger init` records the task's source branch tip even when the primary checkout has another HEAD. Missing recorded sources fail before creating evidence; journey selection still uses the target repository.

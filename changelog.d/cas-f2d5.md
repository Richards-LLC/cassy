### Fixed — Releases

- **Prepared release retries use the verified CLI** — Retrying a prepared release downloads the preflight CLI from the same verified run into a fresh directory. A leftover file no longer makes the preview fall back to the installed CLI.

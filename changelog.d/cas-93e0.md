### Fixed — Factory

- **Shell syntax checks** — Workers can validate literal checkout scripts using only leading `-n` or `--noexec` flags. Syntax checks with redirects or any other option retain the nested shell scope guard, including forms that disable noexec and execute a payload.

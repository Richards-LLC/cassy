### Fixed — Factory

- **Shell syntax checks** — Workers can validate literal checkout scripts with `-n` or `--noexec`, including an optional `--`, without a nested shell scope refusal. Executable command strings and output paths keep their existing guards.

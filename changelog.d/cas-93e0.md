### Fixed — Factory

- **Shell syntax checks** — Workers can validate plain, unquoted checkout script paths using only exact leading `-n` or `--noexec` flags. Redirects, other options, quoting, escaping and shell expansions retain the nested shell scope guard, including forms that disable noexec and execute a payload.

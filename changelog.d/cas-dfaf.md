### Fixed — Factory

- **Reproduce CI with an isolated test HOME** — Named capped worker tests now accept literal HOME, Cargo/Rustup home, CI and Git identity/config prefixes, including `env -u`. Values reach the test child through foreground and detached execution while runner identity, builder admission, private targets and receipts stay protected. Broad test commands and identity/build overrides remain refused.

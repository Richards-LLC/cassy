### Fixed — CLI

- **`cas config set ... --store here` no longer claims CAS_ROOT wins** — Was: with CAS_ROOT naming another project, an explicit `--store here` wrote the working-directory store but still printed that CAS_ROOT supplies the default store and told you to clear the variable. Now: an explicit `--store here` or `--store cas-root` settles the choice and the notice stays silent; a command without `--store` still prints the notice and still refuses an ambiguous write.

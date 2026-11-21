### Fixed — Worker proof

- **Capped temporary directory isolation** — The runner now refuses a selected
  TMPDIR with a physical `.git` ancestor using H-50, even when Git discovery is
  stopped by a ceiling or the scratch path enters a checkout through a symlink.
  Unsafe directories cannot be exported to Cargo and test children.

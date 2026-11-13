### Fixed — Release delivery

- **Check supervisor train pushes** — Supervisor commits run fast admission checks, release-notes lint and Commander dist checks before pushing. Integration publication reuses checked delivery content through an exact head/base marker, avoiding a second fast-row run and the merge push timeout. Doctor and supervisor startup detect missing hook directories.

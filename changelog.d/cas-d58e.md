### Fixed — Release delivery

- **Skip prepared release push validation** — Prepared release commits use the green source receipt named by `Train-CI-Head`, without another train suite or adoption artifact. PR admission, merge queue validation and Release Prebuild continue normally.

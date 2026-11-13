### Fixed — Release delivery

- **Skip prepared release push validation** — Prepared release pushes start no train workflow under freeze-only dispatch. Cut retries use the green source receipt named by `Train-CI-Head` without requiring a run or adoption artifact at the release head. PR admission, merge queue validation and Release Prebuild continue normally.

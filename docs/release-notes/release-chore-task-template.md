# Release chore task template (retired)

Releases no longer need a tag hand-off (cas-f95b). `scripts/release.sh` commits
the version bump, CHANGELOG section and release-notes draft to the epic PR,
and `release.yml` creates the annotated tag and the GitHub release when that
PR lands on `main`. No worker or supervisor pushes a release tag, so there is
no release chore to assign.

See `cas-cli/docs/CONTRIBUTING.md` (Steps to cut a release) and the
`cas-cut-release` skill.

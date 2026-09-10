# CI-to-release-gate exemptions

This file is the explicit exception list for protected-default CI jobs that
do not have a release-gate row. Each future exemption must use one line in the
form `job-id | reason`, with a concrete reason for why the protected lane is
not represented by a release check.

There are currently no exemptions. The required work-producing lanes map to
`archive-mode`, `nextest`, `doctests`, and `macos-check` in
`scripts/check-ci-gate-row-parity.sh`.

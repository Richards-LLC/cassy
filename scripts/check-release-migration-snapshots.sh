#!/usr/bin/env bash
#
# Runs the component-output snapshots when a release includes a registered
# database migration.  This is intentionally a release guard rather than a
# general test wrapper: the snapshots contain the schema/ledger counts that a
# migration changes, and scoped release suites do not compile this test file.

set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

migration_registry="cas-cli/src/migration/migrations/mod.rs"
cargo_bin="${CARGO:-cargo}"

if ! previous_tag="$(git describe --tags --abbrev=0 HEAD 2>/dev/null)"; then
  echo "release guard: no previous tag is reachable; running migration snapshots conservatively"
elif git diff --quiet "$previous_tag"..HEAD -- "$migration_registry"; then
  echo "release guard: $migration_registry is unchanged since $previous_tag; migration snapshots not required"
  exit 0
else
  echo "release guard: $migration_registry changed since $previous_tag; running migration snapshots"
fi

echo "+ cargo test -p cas --test component_output_test"
"$cargo_bin" test -p cas --test component_output_test

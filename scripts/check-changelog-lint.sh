#!/usr/bin/env bash
# Lint the whole CHANGELOG.md with the exact markdownlint policy Docs Lint uses.
#
# Docs Lint runs only for docs-only diffs, so CHANGELOG debt added alongside
# code goes unchecked until a later docs-only change (typically the release
# prep PR) touches the file and the whole backlog fails at once. The release
# train runs this before that PR exists; humans can run it any time.
#
# Usage: scripts/check-changelog-lint.sh [repo-root]
#   CAS_CHANGELOG_LINT_CMD  override the linter command (tests); it receives
#                           `--config <root>/.markdownlint-cli2.jsonc <root>/CHANGELOG.md`
# The linter is the pinned root devDependency, resolved by scripts/markdownlint.py,
# the same one Docs Lint runs.
# Exit: 0 clean, 1 findings, 2 linter unavailable or inputs missing.
set -euo pipefail

root="${1:-$(git rev-parse --show-toplevel)}"
changelog="$root/CHANGELOG.md"
config="$root/.markdownlint-cli2.jsonc"
for input in "$changelog" "$config"; do
    if [[ ! -r "$input" ]]; then
        echo "error: missing $input" >&2
        exit 2
    fi
done

if [[ -n "${CAS_CHANGELOG_LINT_CMD:-}" ]]; then
    read -r -a linter <<<"$CAS_CHANGELOG_LINT_CMD"
else
    linter=(python3 "$(dirname "${BASH_SOURCE[0]}")/markdownlint.py")
fi

status=0
"${linter[@]}" --config "$config" "$changelog" || status=$?
if [[ "$status" == 3 ]]; then
    echo "error: the pinned markdownlint-cli2 is unavailable (see above)" >&2
    exit 2
fi
if [[ "$status" == 0 ]]; then
    echo "ok: CHANGELOG.md passes the Docs Lint markdownlint policy"
    exit 0
fi
echo "error: CHANGELOG.md fails the Docs Lint markdownlint policy (findings above);" \
    "a docs-only change to it would fail Docs Lint" >&2
exit 1

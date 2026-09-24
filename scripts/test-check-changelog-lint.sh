#!/usr/bin/env bash
# Tests for scripts/check-changelog-lint.sh (cas-dd3a).
#
# Keeps the whole CHANGELOG.md clean under the Docs Lint markdownlint policy,
# so a docs-only change to it (the release prep PR) cannot fail on old debt.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
check="$script_dir/check-changelog-lint.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

pass=0
fail=0
ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }

# 1. The check pins the same markdownlint-cli2 as the Docs Lint job.
check_version="$(sed -n 's/^MARKDOWNLINT_CLI2_VERSION="\(.*\)"$/\1/p' "$check")"
ci_version="$(grep -o 'markdownlint-cli2@[0-9.]*' "$repo_root/.github/workflows/ci.yml" | head -n 1 | cut -d@ -f2)"
if [[ -n "$check_version" && "$check_version" == "$ci_version" ]]; then
    ok "check and Docs Lint pin markdownlint-cli2@$ci_version"
else
    bad "check pins '$check_version' but Docs Lint pins '$ci_version'"
fi

fixture() {
    local dir="$tmp/$1"
    mkdir -p "$dir"
    cp "$repo_root/.markdownlint-cli2.jsonc" "$dir/"
    printf '%s' "$2" > "$dir/CHANGELOG.md"
    printf '%s\n' "$dir"
}

# 2. Exit codes and output with a stubbed linter.
stub_dir="$(fixture stub '# Changelog\n')"
printf '#!/usr/bin/env bash\necho "stub finding: $*"\nexit 1\n' > "$tmp/failing-linter"
printf '#!/usr/bin/env bash\nexit 0\n' > "$tmp/passing-linter"
chmod +x "$tmp/failing-linter" "$tmp/passing-linter"
status=0
out="$(CAS_CHANGELOG_LINT_CMD="$tmp/failing-linter" "$check" "$stub_dir" 2>&1)" || status=$?
if [[ "$status" == 1 && "$out" == *"stub finding: --config $stub_dir/.markdownlint-cli2.jsonc $stub_dir/CHANGELOG.md"* \
    && "$out" == *'would fail Docs Lint'* ]]; then
    ok 'findings exit 1, pass the repo config and CHANGELOG.md, and name the Docs Lint consequence'
else
    bad "failing linter: status=$status out=$out"
fi
status=0
out="$(CAS_CHANGELOG_LINT_CMD="$tmp/passing-linter" "$check" "$stub_dir" 2>&1)" || status=$?
if [[ "$status" == 0 && "$out" == *'passes the Docs Lint markdownlint policy'* ]]; then
    ok 'a clean lint exits 0'
else
    bad "passing linter: status=$status out=$out"
fi
missing="$tmp/missing"
mkdir -p "$missing"
status=0
out="$(CAS_CHANGELOG_LINT_CMD="$tmp/passing-linter" "$check" "$missing" 2>&1)" || status=$?
if [[ "$status" == 2 && "$out" == *'missing'* ]]; then
    ok 'missing inputs exit 2 (unavailable), not 1 (findings)'
else
    bad "missing inputs: status=$status out=$out"
fi

# 3. The real linter: it catches the historical debt shape, and the repo's
#    CHANGELOG.md is clean. Skipped only when Node is not installed.
if command -v npx >/dev/null 2>&1; then
    debt_dir="$(fixture debt $'# Changelog\n\n## [0.5.1]\n### Fixed\n- a fix\n')"
    status=0
    out="$("$check" "$debt_dir" 2>&1)" || status=$?
    if [[ "$status" == 1 && "$out" == *'MD022'* && "$out" == *'MD032'* ]]; then
        ok 'the real linter reports the MD022/MD032 shape of the old CHANGELOG debt'
    else
        bad "debt fixture: status=$status out=$out"
    fi
    status=0
    out="$("$check" "$repo_root" 2>&1)" || status=$?
    if [[ "$status" == 0 ]]; then
        ok "the repository CHANGELOG.md has 0 findings under the Docs Lint policy"
    else
        bad "repository CHANGELOG.md has markdownlint findings: $out"
    fi
else
    printf 'skip real-linter checks: npx is not on PATH\n'
fi

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[[ "$fail" == 0 ]]

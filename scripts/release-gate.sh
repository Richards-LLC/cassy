#!/usr/bin/env bash
# Mechanical, fail-closed release train for an assembled epic worktree.
#
# The version bump, Cargo.lock refresh, CHANGELOG section, and Slack draft must
# already be committed on the source branch. This gate proves the tree before
# it enters the merge queue; it does not mutate release metadata. A receipt is
# printed even when one check fails so the failure can be pasted into the epic
# close note without relying on supervisor memory.
#
# Usage: scripts/release-gate.sh <version>

set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

failure_log_rel='cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md'
failure_log_codex_rel='cas-cli/src/builtins/codex/skills/cas-cut-release/references/failure-log.md'
failure_log_grok_rel='cas-cli/src/builtins/grok/skills/cas-cut-release/references/failure-log.md'
readonly -a gate_check_ids=(
    version-literals workspace-tests nextest doctests archive-mode
    snapshot-portability builtin-projections changelog-and-versions
    working-tree release-script procedure-guardrails failure-log
    ancestor-proxy-config
)

usage() {
    printf 'Usage: %s <version>\n' "$0"
    printf '       %s --learn "<symptom>" "<cause>" "<check-id>"\n' "$0"
}

learn() {
    local symptom="$1" cause="$2" check_id="$3"
    local date entry path before
    [[ "$symptom" != *$'\n'* && "$cause" != *$'\n'* ]] || {
        printf 'error: --learn values must be single-line strings\n' >&2
        return 2
    }
    [[ "$check_id" =~ ^(manual:)?[a-z0-9-]+$ ]] || {
        printf 'error: invalid check id %s\n' "$check_id" >&2
        return 2
    }
    date="$(date -u +%F)"
    entry="- $date — **$check_id** — Symptom: $symptom Root cause: $cause Release: operator-reported."
    for path in "$failure_log_rel" "$failure_log_codex_rel" "$failure_log_grok_rel"; do
        [[ -f "$path" ]] || {
            printf 'error: missing failure-log mirror %s\n' "$path" >&2
            return 1
        }
    done
    for path in "$failure_log_rel" "$failure_log_codex_rel" "$failure_log_grok_rel"; do
        before="$(mktemp)"
        cp "$path" "$before"
        printf '%s\n' "$entry" >>"$path"
        printf '%s\n' "--- $path"
        if ! diff -u "$before" "$path"; then
            :
        fi
        rm -f "$before"
    done
    printf 'Learned release failure in all three mirrors. Add or extend check %s and its fixture self-test in this same commit; record the same text with mcp__cas__memory action=remember tags=release before retrying.\n' "$check_id"
}

if [[ "${1:-}" == '--learn' ]]; then
    [[ "$#" -eq 4 ]] || {
        usage >&2
        exit 2
    }
    learn "$2" "$3" "$4"
    exit $?
fi

if [[ "$#" -ne 1 || ! "${1:-}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    usage >&2
    exit 2
fi

version="$1"
repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

cargo_bin="${CARGO:-cargo}"
reference_history_script="${RELEASE_GATE_GEN_REFERENCE_HISTORY:-$repo_root/scripts/gen-builtin-reference-history.sh}"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/cas-release-gate.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

failures=()

print_result() {
    local status="$1" name="$2" command="$3"
    printf '%s %s — %s\n' "$status" "$name" "$command"
}

run_check() {
    local name="$1" command="$2" function_name="$3" log status
    log="$tmp_dir/$name.log"
    if "$function_name" >"$log" 2>&1; then
        print_result PASS "$name" "$command"
    else
        status=$?
        print_result FAIL "$name" "$command"
        failures+=("$name")
        if [[ -s "$log" ]]; then
            sed 's/^/  | /' "$log" | tail -20
        fi
        printf '  | exit status: %s\n' "$status"
    fi
}

is_gate_check_id() {
    local candidate="$1" known
    for known in "${gate_check_ids[@]}"; do
        [[ "$candidate" == "$known" ]] && return 0
    done
    return 1
}

check_failure_log() {
    local log="$failure_log_rel"
    local mirror entry id
    local entries=0 enforced=0 manual=0 invalid=0
    [[ -f "$log" ]] || {
        printf 'failure-log: missing %s\n' "$log"
        return 1
    }
    while IFS= read -r entry || [[ -n "$entry" ]]; do
        [[ -z "$entry" ]] && continue
        entries=$((entries + 1))
        if [[ "$entry" == *manual:* ]]; then
            manual=$((manual + 1))
        elif [[ "$entry" =~ \*\*([a-z0-9-]+)\*\* ]]; then
            id="${BASH_REMATCH[1]}"
            if is_gate_check_id "$id"; then
                enforced=$((enforced + 1))
            else
                invalid=$((invalid + 1))
                printf 'failure-log: unknown executable check id %s\n' "$id"
            fi
        else
            invalid=$((invalid + 1))
            printf 'failure-log: entry has no **check-id** or manual: marker: %s\n' "$entry"
        fi
    done <"$log"
    for mirror in "$failure_log_codex_rel" "$failure_log_grok_rel"; do
        if ! cmp -s "$log" "$mirror"; then
            invalid=$((invalid + 1))
            printf 'failure-log: mirror differs: %s\n' "$mirror"
        fi
    done
    printf 'failure-log: %d entries enforced; %d entries with no executable check (explicit manual markers); %d invalid\n' \
        "$enforced" "$manual" "$invalid"
    [[ "$entries" -gt 0 && "$invalid" -eq 0 ]]
}

check_version_literals() {
    local file hit found=false
    local -a roots=(cas-cli/src cas-cli/tests crates scripts)

    for root in "${roots[@]}"; do
        [[ -d "$root" ]] || continue
        while IFS= read -r -d '' file; do
            case "$file" in
                *.md|*/reference-history.json|*/failure-log.md|*/Cargo.toml|*/Cargo.lock) continue ;;
            esac
            hit="$(grep -nIF -- "$version" "$file" 2>/dev/null || true)"
            if [[ -n "$hit" ]]; then
                printf '%s\n' "$hit"
                found=true
            fi
        done < <(find "$root" -type f -print0)
    done

    if [[ "$found" == true ]]; then
        printf 'version-literals: source/test files contain %s; use env!("CARGO_PKG_VERSION") or a fixture value\n' "$version"
        return 1
    fi
}

check_workspace_tests() {
    "$cargo_bin" check --workspace --tests
}

check_nextest() {
    "$cargo_bin" nextest run -p cas
}

check_doctests() {
    "$cargo_bin" test -p cas --doc
}

# A populated proxy.toml in an ancestor .cas is visible to any test that
# resolves project config by walking up from its cwd — release worktrees live at
# <repo>/.cas/worktrees/<name>, so the main checkout is always an ancestor.
#
# On 2026-09-03 that made three hermetic proxy tests fail during the v3.14.0
# gate. The suite is now immune (TestEnvGuard pins CAS_ROOT inside its temp
# HOME, cas-4ccc); this row covers whatever does not use that guard.
#
# The gate neutralizes rather than refuses: a release must not be blocked by the
# operator's own MCP configuration, and telling a human to move their files
# aside is how the original hour was lost. `CAS_ROOT` is the loader's documented
# override and wins ahead of both the worktree mapping and the ancestor walk, so
# pointing it at an empty directory makes the ancestor file unreachable for
# every child process. The file is named in the receipt either way, and never
# touched.
ancestor_proxy_config_files() {
    local probe files=()
    probe="$(cd "$repo_root" && pwd -P)"
    while [[ "$probe" != "/" && -n "$probe" ]]; do
        if [[ -s "$probe/.cas/proxy.toml" && "$probe/.cas" != "$repo_root/.cas" ]]; then
            files+=("$probe/.cas/proxy.toml")
        fi
        probe="$(dirname "$probe")"
    done
    (( ${#files[@]} )) && printf '%s\n' "${files[@]}"
    return 0
}

neutralize_ancestor_proxy_config() {
    local files=()
    while IFS= read -r line; do [[ -n "$line" ]] && files+=("$line"); done \
        < <(ancestor_proxy_config_files)
    (( ${#files[@]} )) || return 0

    hermetic_cas_root="$tmp_dir/hermetic-cas-root"
    mkdir -p "$hermetic_cas_root"
    export CAS_ROOT="$hermetic_cas_root"
    local file
    for file in "${files[@]}"; do
        printf 'note: ancestor .cas/proxy.toml visible to this worktree: %s\n' "$file"
    done
    printf 'note: running with CAS_ROOT=%s so ancestor-walking tests cannot read it\n' \
        "$hermetic_cas_root"
}

check_ancestor_proxy_config() {
    local files=()
    while IFS= read -r line; do [[ -n "$line" ]] && files+=("$line"); done \
        < <(ancestor_proxy_config_files)
    if (( ${#files[@]} == 0 )); then
        printf 'no ancestor .cas/proxy.toml above this worktree\n'
        return 0
    fi
    # Present, so the override must be in force and itself empty.
    if [[ -z "${CAS_ROOT:-}" || ! -d "${CAS_ROOT:-}" || -s "${CAS_ROOT:-}/proxy.toml" ]]; then
        printf 'ancestor .cas/proxy.toml is readable and CAS_ROOT is not pinned to an empty root: %s\n' \
            "${files[*]}"
        return 1
    fi
    printf 'neutralized %s ancestor proxy.toml file(s) with CAS_ROOT=%s: %s\n' \
        "${#files[@]}" "$CAS_ROOT" "${files[*]}"
    return 0
}

# Scratch bases must mirror the merge-queue runner: no ancestor directory may
# hold a .cas store, or every cas child that walks up from its cwd/TMPDIR
# finds the host's user-level store instead of the fixture.
assert_no_cas_ancestor() {
    local base="$1" probe
    probe="$(cd "$(dirname "$base")" 2>/dev/null && pwd -P || printf '%s' "$(dirname "$base")")"
    while [[ "$probe" != "/" && -n "$probe" ]]; do
        if [[ -d "$probe/.cas" ]]; then
            printf 'scratch base %s has a .cas ancestor at %s; set CAS_RELEASE_GATE_HOME_DIR to a path with no .cas ancestor (the queue runner has none)\n' "$base" "$probe/.cas"
            return 1
        fi
        probe="$(dirname "$probe")"
    done
    [[ -d "/.cas" ]] && { printf 'scratch base %s has a .cas ancestor at /.cas\n' "$base"; return 1; }
    return 0
}

make_archive_path() {
    local name command
    for name in cargo rustc git sh bash jq python3; do
        command="$(command -v "$name" || true)"
        [[ -n "$command" ]] || {
            printf 'archive-mode: required archive command is missing: %s\n' "$name"
            return 1
        }
        ln -s "$command" "$archive_bin/$name"
    done
    printf '/usr/bin:/bin'
}

check_archive_mode() {
    local archive_base archive_dir archive remap archive_tmp archive_path manifest package_dir status
    archive_base="${CAS_RELEASE_GATE_HOME_DIR:-${HOME:?}/.cache/cas-release-gate}"
    mkdir -p "$(dirname "$archive_base")"
    assert_no_cas_ancestor "$archive_base" || return 1
    archive_dir="$(mktemp -d "${archive_base}.XXXXXX")"
    archive="$archive_dir/suite.tar.zst"
    remap="$archive_dir/workspace-remap"
    archive_tmp="$archive_dir/tmp"
    archive_bin="$archive_dir/bin"
    mkdir -p "$remap" "$archive_tmp" "$archive_bin"
    [[ -f Cargo.toml ]] || {
        printf 'archive-mode: root Cargo.toml is missing\n'
        return 1
    }
    # Mirror the merge-queue shard runner exactly: it HAS a checkout of the
    # tree, but at a different path than the build host, so compile-time
    # CARGO_MANIFEST_DIR reads fail while cwd-relative reads still work.
    # Empty package directories were stricter than CI and rejected a
    # pre-existing cwd-relative source inspection.
    # A detached git worktree, not `git archive`: the runner's checkout has a
    # .git, and tests that call git (check-ignore, rev-parse) need it.
    rmdir "$remap" 2>/dev/null || true
    git worktree add --detach "$remap" HEAD >/dev/null 2>&1 || {
        printf 'archive-mode: cannot create remap worktree at %s\n' "$remap"
        return 1
    }
    archive_path="$(make_archive_path)"
    if "$cargo_bin" nextest archive -p cas --archive-file "$archive"; then
        :
    else
        status=$?
        git worktree remove --force "$remap" >/dev/null 2>&1 || true
        rm -rf "$archive_dir"
        return "$status"
    fi
    [[ -s "$archive" ]] || {
        printf 'archive-mode: nextest did not create %s\n' "$archive"
        return 1
    }
    # Extraction is deliberately on the home disk, not a small /tmp tmpfs.
    # The remap has every package cwd but no source; snapshot tests read
    # source-tree .snap files and are excluded rather than "fixed".
    if (
        cd "$archive_dir"
        env -u COLUMNS HOME="${HOME:-$archive_dir}" TMPDIR="$archive_tmp" \
            PATH="$archive_bin${archive_path:+:$archive_path}" \
            "$cargo_bin" nextest run --archive-file "$archive" \
            --workspace-remap "$remap" \
            --filterset 'not binary_id(~component_output_test)'
    ); then
        status=0
    else
        status=$?
    fi
    git worktree remove --force "$remap" >/dev/null 2>&1 || true
    rm -rf "$archive_dir"
    return "$status"
}

check_snapshot_portability() {
    # The deep TMPDIR must live OUTSIDE every checkout: worktrees sit under the
    # main repo's .cas/, so a temp dir inside the tree lets find_cas_root walk
    # up into the real project store and the snapshot captures live data.
    local deep_base="${CAS_RELEASE_GATE_HOME_DIR:-${HOME:?}/.cache/cas-release-gate}"
    mkdir -p "$(dirname "$deep_base")"
    assert_no_cas_ancestor "$deep_base" || return 1
    local deep_tmp
    deep_tmp="$(mktemp -d "${deep_base}.snap.XXXXXX")/$(printf 'deep-temp-path-%.0s' {1..12})"
    mkdir -p "$deep_tmp"
    # COLUMNS must be absent, rather than merely empty: terminal-width probes
    # commonly distinguish the two states.
    local status
    env -u COLUMNS INSTA_UPDATE=no TMPDIR="$deep_tmp" \
        "$cargo_bin" nextest run -p cas --test component_output_test
    status=$?
    # Never leave insta's pending-snapshot artifacts behind: they would fail
    # the working-tree row of this same gate.
    find . -path ./target -prune -o -name '*.snap.new' -print0 2>/dev/null | xargs -0 rm -f --
    return "$status"
}

check_builtin_projections() {
    "$cargo_bin" nextest run -p cas --test builtin_flavor_drift_test \
        root_managed_projections_stay_synced_and_project_skills_stay_ignored || return $?
    [[ -x "$reference_history_script" ]] || {
        printf 'builtin-projections: missing executable %s\n' "$reference_history_script"
        return 1
    }
    "$reference_history_script" || return $?
    if ! git diff --quiet -- cas-cli/src/builtins/reference-history.json; then
        printf 'reference-history: regenerated ledger differs from the committed file\n'
        git diff -- cas-cli/src/builtins/reference-history.json
        return 1
    fi
}

check_changelog_and_versions() {
    local file current
    local -a release_crates=(
        cas-cli/Cargo.toml
        crates/cas-types/Cargo.toml
        crates/cas-search/Cargo.toml
        crates/cas-store/Cargo.toml
        crates/cas-core/Cargo.toml
        crates/cas-mcp/Cargo.toml
    )

    grep -Eq '^## \[Unreleased\]$' CHANGELOG.md || {
        printf 'changelog: CHANGELOG.md is missing ## [Unreleased]\n'
        return 1
    }
    grep -Eq "^## \[$version\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$" CHANGELOG.md || {
        printf 'changelog: missing release heading for %s\n' "$version"
        return 1
    }
    if ! awk -v version="$version" '
        $0 ~ "^## \\[" version "\\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$" { in_section=1; next }
        in_section && /^## \[/ { in_section=0 }
        in_section && /^- / { found=1 }
        END { exit(found ? 0 : 1) }
    ' CHANGELOG.md; then
        printf 'changelog: section [%s] has no bullet\n' "$version"
        return 1
    fi

    for file in "${release_crates[@]}"; do
        [[ -f "$file" ]] || {
            printf 'version-alignment: missing %s\n' "$file"
            return 1
        }
        current="$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$file" | head -n1)"
        if [[ "$current" != "$version" ]]; then
            printf 'version-alignment: %s is %s; expected %s\n' "$file" "${current:-missing}" "$version"
            return 1
        fi
    done
}

check_release_script() {
    [[ -f scripts/release.sh ]] || {
        printf 'release-script: scripts/release.sh is missing\n'
        return 1
    }
    grep -qF 'target/$target/release/build"/blake3-*' scripts/release.sh
    grep -qF 'target/$target/release/.fingerprint"/blake3-*' scripts/release.sh
    grep -qF 'Pre-warming rule: in a tag worktree' scripts/release.sh
    grep -qF 'audit-only and remote-safe' scripts/release.sh
}

check_procedure_guardrails() {
    local skill="cas-cli/src/builtins/skills/cas-cut-release/SKILL.md"
    [[ -f "$skill" ]] || {
        printf 'procedure-guardrails: missing %s\n' "$skill"
        return 1
    }
    grep -qF 'kill -0' "$skill"
    grep -qF 'full suite on the assembled tree' "$skill"
    grep -qF 'stranded_branch_override' "$skill"
    grep -qF 'release-published-receipt.sh --write-draft' "$skill"
    grep -qF 'cas --version' "$skill"
}

check_working_tree() {
    local untracked
    if ! git diff --quiet; then
        git diff --stat
        printf 'working-tree: unstaged changes are present\n'
        return 1
    fi
    if ! git diff --cached --quiet; then
        git diff --cached --stat
        printf 'working-tree: staged changes are present\n'
        return 1
    fi
    untracked="$(git ls-files --others --exclude-standard)"
    if [[ -n "$untracked" ]]; then
        printf '%s\n' "$untracked"
        printf 'working-tree: untracked files are present\n'
        return 1
    fi
}

printf '=== CAS RELEASE GATE RECEIPT ===\n'
printf 'version: %s\n' "$version"
printf 'repository: %s\n' "$repo_root"
neutralize_ancestor_proxy_config

run_check failure-log \
    "parse $failure_log_rel; every entry maps to a gate check id or manual:" \
    check_failure_log
run_check ancestor-proxy-config \
    'no populated .cas/proxy.toml above this worktree that ancestor-walking tests could read' \
    check_ancestor_proxy_config
run_check version-literals \
    'find source/test files for <version> (excluding manifests, CHANGELOG, reference-history, failure-log)' \
    check_version_literals
run_check workspace-tests \
    "$cargo_bin check --workspace --tests" \
    check_workspace_tests
run_check nextest \
    "$cargo_bin nextest run -p cas" \
    check_nextest
run_check doctests \
    "$cargo_bin test -p cas --doc" \
    check_doctests
run_check archive-mode \
    "$cargo_bin nextest archive -p cas --archive-file <home-disk>/suite.tar.zst; archive run outside checkout with remap and rg removed" \
    check_archive_mode
run_check snapshot-portability \
    'env -u COLUMNS TMPDIR=<deep path> cargo nextest run -p cas --test component_output_test' \
    check_snapshot_portability
run_check builtin-projections \
    "$cargo_bin nextest run -p cas --test builtin_flavor_drift_test root_managed_projections...; regenerate ledger; git diff --quiet" \
    check_builtin_projections
run_check changelog-and-versions \
    'CHANGELOG [Unreleased]/[version] bullet plus six release-train Cargo.toml versions' \
    check_changelog_and_versions
run_check release-script \
    'release.sh stale duplicate cleanup and audit-only pre-warm contract' \
    check_release_script
run_check procedure-guardrails \
    'cas-cut-release reconciliation, queue, PID, receipt, and host guardrails' \
    check_procedure_guardrails
run_check working-tree \
    'git diff --quiet; git diff --cached --quiet; git ls-files --others --exclude-standard' \
    check_working_tree

if [[ "${#failures[@]}" -gt 0 ]]; then
    printf 'RELEASE GATE FAILED: %s\n' "${failures[*]}"
    exit 1
fi

printf 'RELEASE GATE PASSED: all checks are green for %s\n' "$version"

#!/usr/bin/env bash
#
# Regression guard for cas-78c8 / GH #156: prove a test run writes nothing into
# a real CAS store.
#
# For several months the integration suite wrote its fixture memories straight
# into the developer's `~/.cas/cas.db` and the cas-src project database — 994
# junk rows, 58.6% of the corpus, indistinguishable at a glance from real
# memories. Nothing detected it because nothing was looking.
#
# This script looks, two independent ways:
#
#   1. Tripwire (fail-fast). It exports CAS_TEST_PROTECTED_DBS naming both real
#      databases. `cas_store::shared_db::shared_connection` — the single choke
#      point every production store open funnels through — panics the instant a
#      test process or a `cas` subprocess it spawned tries to open one. This
#      catches reads as well as writes, and names the offending test.
#
#   2. Row-count diff (fail-safe). Counts every row of every table in both
#      databases before and after the run and reports any drift. This catches a
#      leak that somehow bypasses the tripwire (a raw `rusqlite::Connection`,
#      a shell-out to `sqlite3`) at the cost of only noticing after the fact.
#
# The two are not redundant, and only the first is attributable. The row-count
# diff cannot tell a leaking test apart from *any other writer* — on a machine
# running CAS factory agents, the project store gains `events` and
# `supervisor_queue` rows continuously while the suite runs, and the diff will
# report that as drift. Set CAS_STORE_GUARD_IGNORE_DRIFT=1 to downgrade the
# row-count half to a warning when knowingly running beside live agents; on a
# clean CI runner leave it unset, where any drift is real.
#
# Usage:
#   scripts/check-real-store-untouched.sh                    # full suite
#   scripts/check-real-store-untouched.sh --test cli_test    # scoped run
#
# Any arguments are passed through to `cargo test`. Scoping is strongly
# encouraged for routine use — a full `cargo test` in this repo links ~64 test
# binaries and is expensive (see CLAUDE.md).
#
# Exit codes: 0 = clean, 1 = drift detected or the test run failed.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO="${CARGO:-cargo}"

# The real stores this run must not touch. CAS_REAL_DBS overrides for
# environments whose stores live elsewhere (colon-separated).
if [[ -n "${CAS_REAL_DBS:-}" ]]; then
    IFS=':' read -r -a REAL_DBS <<<"${CAS_REAL_DBS}"
else
    REAL_DBS=("${HOME}/.cas/cas.db" "${REPO_ROOT}/.cas/cas.db")
fi

# A CAS worktree shares the parent repo's .cas directory, so REPO_ROOT/.cas may
# not exist here while the real project store sits above it. Resolve it the way
# `find_cas_root` does rather than silently guarding nothing.
if [[ "${REPO_ROOT}" == *"/.cas/worktrees/"* ]]; then
    parent_cas="${REPO_ROOT%%/.cas/worktrees/*}/.cas/cas.db"
    REAL_DBS+=("${parent_cas}")
fi

present_dbs=()
for db in "${REAL_DBS[@]}"; do
    [[ -f "${db}" ]] && present_dbs+=("${db}")
done

if [[ ${#present_dbs[@]} -eq 0 ]]; then
    echo "No real CAS store found at: ${REAL_DBS[*]}"
    echo "Nothing to protect — running the tests without a guard would prove nothing."
    exit 0
fi

# Total row count across every user table, so the guard is not blind to a leak
# that lands in tasks/rules/events rather than entries.
snapshot() {
    local db="$1"
    sqlite3 "file:${db}?mode=ro" \
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%';" 2>/dev/null |
        while read -r table; do
            local count
            count="$(sqlite3 "file:${db}?mode=ro" "SELECT COUNT(*) FROM \"${table}\";" 2>/dev/null)"
            printf '%s\t%s\n' "${table}" "${count:-ERROR}"
        done | sort
}

if ! command -v sqlite3 >/dev/null 2>&1; then
    echo "error: sqlite3 is required for the row-count half of this guard" >&2
    exit 1
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

echo "Protecting ${#present_dbs[@]} real store(s):"
for db in "${present_dbs[@]}"; do
    echo "  - ${db}"
    snapshot "${db}" >"${tmpdir}/before-$(echo "${db}" | tr '/' '_')"
done

protected="$(
    IFS=':'
    echo "${present_dbs[*]}"
)"
export CAS_TEST_PROTECTED_DBS="${protected}"

echo
echo "Running: ${CARGO} test $*"
echo "CAS_TEST_PROTECTED_DBS=${CAS_TEST_PROTECTED_DBS}"
echo
(cd "${REPO_ROOT}" && "${CARGO}" test "$@")
test_status=$?

echo
drift=0
for db in "${present_dbs[@]}"; do
    key="$(echo "${db}" | tr '/' '_')"
    snapshot "${db}" >"${tmpdir}/after-${key}"
    if ! diff -u "${tmpdir}/before-${key}" "${tmpdir}/after-${key}" >"${tmpdir}/diff-${key}"; then
        drift=1
        echo "DRIFT in ${db}:"
        cat "${tmpdir}/diff-${key}"
    else
        echo "clean: ${db} (no row-count change in any table)"
    fi
done

if [[ ${drift} -ne 0 ]]; then
    echo
    if [[ "${CAS_STORE_GUARD_IGNORE_DRIFT:-0}" == "1" ]]; then
        echo "WARNING: a real CAS store changed during the run, but CAS_STORE_GUARD_IGNORE_DRIFT=1"
        echo "attributes it to concurrent writers rather than the tests. The tripwire above did not"
        echo "fire, which is the attributable signal — but read the diff before believing it."
    else
        echo "FAIL: the test run changed a real CAS store. Either a test escaped its sandbox"
        echo "(anchor it to a temp directory — cas-cli/tests/support/mod.rs::CasSandbox), or"
        echo "another process wrote to the store while the suite ran. On a machine with live"
        echo "CAS agents the latter is expected; re-run with CAS_STORE_GUARD_IGNORE_DRIFT=1"
        echo "and rely on the tripwire, which names the offending test."
        exit 1
    fi
fi

if [[ ${test_status} -ne 0 ]]; then
    echo
    echo "Real stores are clean, but the test run itself failed (exit ${test_status})."
    exit "${test_status}"
fi

echo
echo "PASS: real stores untouched."

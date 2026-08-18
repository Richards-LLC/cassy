#!/usr/bin/env bash
# Publish this job's sccache effectiveness to the GitHub job summary.
#
# Why (cas-67a2, rank-3 of docs/analysis/2026-08-17-ci-speed-spike.md): warm CI
# lanes reach 90-92% compiler-cache hits, but that number is invisible unless a
# human greps a job log, so a lane that silently regresses to the pre-cache-v2
# 0-6% range — or pays a 43% cold/seed penalty — costs minutes with nobody
# noticing. Every compiling Rust lane calls this at the end of the job.
#
# OBSERVABILITY MUST NEVER FAIL A BUILD. This script deliberately runs without
# `set -e` and always exits 0: a broken stats probe is a reporting bug, not a
# reason to redden a lane that compiled and tested fine.
#
# sccache-action's own post hook emits a bare one-line notice annotation. This
# is not that: it renders a table into the job summary, names the backend the
# lane actually resolved, warns on a cold or unconfigured lane, and still says
# something useful when sccache is missing or its stats are unreadable.
#
# Usage: scripts/ci-sccache-summary.sh "<lane label>"

set -uo pipefail

lane="${1:-${GITHUB_JOB:-unknown lane}}"
# A lane whose hit rate falls below this is cold or misconfigured; annotate it
# so the cost is visible in the run's Annotations, not just in this summary.
min_hit_rate="${CAS_SCCACHE_MIN_HIT_RATE:-50}"
# Below this many compile requests the ratio is noise (a lane that short-circuited,
# or one whose whole graph was already linked), so no warning is raised.
min_requests_for_warning="${CAS_SCCACHE_MIN_REQUESTS:-10}"

emit() {
    printf '%s\n' "$1"
    if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
        printf '%s\n' "$1" >>"$GITHUB_STEP_SUMMARY"
    fi
}

# A log line alone cannot distinguish "rendered into the job summary" from
# "printed to stdout", so state which one happened. Every caller ends with this.
report_destination() {
    if [[ -z "${GITHUB_STEP_SUMMARY:-}" ]]; then
        echo "sccache stats printed to the log only: GITHUB_STEP_SUMMARY is unset."
    elif [[ -s "${GITHUB_STEP_SUMMARY}" ]]; then
        echo "sccache stats written to the job summary ($(wc -c <"$GITHUB_STEP_SUMMARY" | tr -d '[:space:]') bytes at $GITHUB_STEP_SUMMARY)."
    else
        echo "sccache stats could not be written to the job summary at $GITHUB_STEP_SUMMARY."
    fi
}

backend_label() {
    if [[ "${SCCACHE_GHA_ENABLED:-}" == "true" || "${SCCACHE_GHA_ENABLED:-}" == "on" ]]; then
        printf 'GitHub Actions cache v2 (namespace `%s`)' "${SCCACHE_GHA_VERSION:-<unset>}"
    elif [[ -z "${SCCACHE_GHA_ENABLED:-}" ]]; then
        printf '**not configured** — `SCCACHE_GHA_ENABLED` is unset, so this lane cannot reuse CI objects'
    else
        printf 'disabled (`SCCACHE_GHA_ENABLED=%s`)' "${SCCACHE_GHA_ENABLED}"
    fi
}

report_unavailable() {
    emit "### sccache — ${lane}"
    emit ""
    emit "Cache statistics unavailable: $1."
    emit ""
    emit "Backend: $(backend_label)"
    echo "::warning title=sccache stats unavailable::${lane}: $1 — this lane compiled without measured cache reuse."
    report_destination
    exit 0
}

# The shared setup's probe writes SCCACHE_GHA_ENABLED=false into GITHUB_ENV when
# the action download or server start failed. In that state the wrapper is
# already cleared and asking the binary for stats would just restart a server
# that nothing used.
if [[ "${SCCACHE_GHA_ENABLED:-}" == "false" ]]; then
    report_unavailable "sccache backend was disabled for this job (build ran uncached)"
fi

if ! command -v sccache >/dev/null 2>&1; then
    report_unavailable "the sccache executable is not on PATH"
fi

json="$(sccache --show-stats --stats-format=json 2>/dev/null)"
text="$(sccache --show-stats 2>/dev/null)"

hits=""
misses=""
requests=""
executed=""
errors=""
writes=""

if [[ -n "$json" ]] && command -v jq >/dev/null 2>&1 && jq -e . >/dev/null 2>&1 <<<"$json"; then
    read -r requests executed hits misses errors writes < <(
        jq -r '
            def total: (.counts // {}) | to_entries | map(.value) | add // 0;
            .stats
            | [ (.compile_requests // 0),
                (.requests_executed // 0),
                (.cache_hits | total),
                (.cache_misses | total),
                (.cache_errors | total),
                (.cache_writes // 0) ]
            | @tsv
        ' <<<"$json" | tr '\t' ' '
    )
elif [[ -n "$text" ]]; then
    # Fallback for an sccache build whose JSON schema this script does not know.
    field() { awk -v key="$1" 'index($0, key) == 1 { print $NF }' <<<"$text" | head -1; }
    requests="$(field 'Compile requests ')"
    executed="$(field 'Compile requests executed')"
    hits="$(field 'Cache hits ')"
    misses="$(field 'Cache misses ')"
    errors="$(field 'Cache errors')"
    writes="$(field 'Cache writes')"
else
    report_unavailable "sccache returned no statistics"
fi

# Any field the probe could not read stays visibly unknown rather than silently 0.
numeric() { [[ "$1" =~ ^[0-9]+$ ]] && printf '%s' "$1" || printf '?'; }
hits="$(numeric "${hits:-}")"
misses="$(numeric "${misses:-}")"
requests="$(numeric "${requests:-}")"
executed="$(numeric "${executed:-}")"
errors="$(numeric "${errors:-}")"
writes="$(numeric "${writes:-}")"

rate="n/a"
rate_value=""
if [[ "$hits" != "?" && "$misses" != "?" ]]; then
    total=$((hits + misses))
    if ((total > 0)); then
        rate_value=$(((hits * 100) / total))
        rate="${rate_value}%"
    fi
fi

emit "### sccache — ${lane}"
emit ""
emit "**${hits} hits / ${misses} misses — hit rate ${rate}** (${executed} of ${requests} compile requests executed)"
emit ""
emit "| Metric | Value |"
emit "| --- | --- |"
emit "| Cache hits | ${hits} |"
emit "| Cache misses | ${misses} |"
emit "| Hit rate | ${rate} |"
emit "| Compile requests | ${requests} |"
emit "| Requests executed | ${executed} |"
emit "| Cache writes | ${writes} |"
emit "| Cache errors | ${errors} |"
emit "| Backend | $(backend_label) |"
emit ""

echo "::notice title=sccache ${lane}::${hits} hits / ${misses} misses (hit rate ${rate}) across ${requests} compile requests."

if [[ -z "${SCCACHE_GHA_ENABLED:-}" ]]; then
    echo "::warning title=sccache backend unconfigured::${lane} compiled without SCCACHE_GHA_ENABLED; CI objects are not being shared."
elif [[ -n "$rate_value" && "$requests" != "?" ]] \
    && ((requests >= min_requests_for_warning)) && ((rate_value < min_hit_rate)); then
    echo "::warning title=sccache cold lane::${lane} hit rate ${rate} is below ${min_hit_rate}% over ${requests} compile requests; this lane paid a cold/seed compile penalty."
fi

report_destination
exit 0

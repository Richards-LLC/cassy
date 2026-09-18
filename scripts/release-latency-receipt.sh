#!/usr/bin/env bash
# Measure the only release number an operator feels: tag push -> published
# release (cas-3b7c0 / GH #449).
#
# The digest receipt (scripts/release-published-receipt.sh) proves *what* was
# published. This proves *how fast*. Both are needed before a release is
# announced as fast: a receipt that only reports digests cannot tell a
# 3-minute prebuilt publication from a 26-minute cold one.
#
# Exits non-zero when the measured latency exceeds the budget, so the number is
# a gate an operator can run rather than a figure to eyeball.
set -euo pipefail

usage() {
    echo "Usage: scripts/release-latency-receipt.sh <vX.Y.Z> [--budget-seconds <n>] [--run-dir <path>]" >&2
}

if [[ "$#" -lt 1 ]]; then
    usage
    exit 2
fi

tag="$1"
if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: expected an annotated release tag like vX.Y.Z; got $tag" >&2
    exit 2
fi

budget=600
run_dir="${CAS_RELEASE_TRAIN_RUN_DIR:-${CAS_RELEASE_RUN_DIR:-}}"
shift
while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --budget-seconds)
            [[ "$#" -ge 2 && "$2" =~ ^[0-9]+$ ]] || { usage; exit 2; }
            budget="$2"
            shift 2
            ;;
        --run-dir)
            [[ "$#" -ge 2 && -n "$2" ]] || { usage; exit 2; }
            run_dir="$2"
            shift 2
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

if [[ -z "$run_dir" ]]; then
    artifacts_root="${CAS_RELEASE_ARTIFACTS_ROOT:-$HOME/.cas/artifacts/release}"
    mapfile -t candidates < <(find "$artifacts_root" -mindepth 1 -maxdepth 1 -type d \
        -name "v${tag#v}-*" -print 2>/dev/null | sort)
    if [[ "${#candidates[@]}" -eq 1 ]]; then
        run_dir="${candidates[0]}"
    fi
fi

intervention_count() {
    local log="${run_dir:-}/interventions.log"
    [[ -s "$log" ]] || { printf '0\n'; return; }
    awk '
        function field(prefix,    i) {
            for (i = 1; i <= NF; i++) if (index($i, prefix) == 1) return substr($i, length(prefix) + 1)
            return ""
        }
        {
            kind = field("kind="); command = field("subcommand="); resume = field("resume="); blockers = field("blockers=")
            if (kind != "manual") next
            if (command == "--cut" && resume == "true" && blockers != "" && blockers != "none") {
                count = split(blockers, names, ",")
                for (i = 1; i <= count; i++) if (names[i] ~ /^(preflight|assemble|prep|ledger|gate|pr-body|pipeline|publish|post-publication|announce|report|receipts|host-update)$/) seen[names[i]]++
            } else count_manual++
        }
        END {
            for (name in seen) count_manual++
            print count_manual + 0
        }
    ' "$log"
}

intervention_blockers() {
    local log="${run_dir:-}/interventions.log" raw='' path base stage
    if [[ -s "$log" ]]; then
        raw="$(awk '
            function field(prefix,    i) {
                for (i = 1; i <= NF; i++) if (index($i, prefix) == 1) return substr($i, length(prefix) + 1)
                return ""
            }
            function canonical(name) { return name ~ /^(preflight|assemble|prep|ledger|gate|pr-body|pipeline|publish|post-publication|announce|report|receipts|host-update)$/ }
            function add(name) { if (canonical(name) && !seen[name]++) names[++n] = name }
            {
                if (field("kind=") != "manual") next
                add(field("stage=")); blockers = field("blockers=")
                if (blockers != "" && blockers != "none") {
                    count = split(blockers, values, ",")
                    for (i = 1; i <= count; i++) add(values[i])
                }
            }
            END {
                for (i = 1; i <= n; i++) printf "%s%s", (i == 1 ? "" : ","), names[i]
            }
        ' "$log")"
    fi
    if [[ -z "$raw" && -s "${run_dir:-}/blockers.log" ]]; then
        raw="$(tr '\n' ',' <"$run_dir/blockers.log" | sed 's/,$//')"
    fi
    if [[ -z "$raw" && -n "$run_dir" ]]; then
        for path in "$run_dir"/stage.*.blocked "$run_dir"/blocker.*; do
            [[ -e "$path" ]] || continue
            base="$(basename "$path")"
            case "$base" in
                stage.*.blocked) stage="${base#stage.}"; stage="${stage%.blocked}" ;;
                blocker.*) stage="${base#blocker.}" ;;
                *) continue ;;
            esac
            [[ -n "$stage" ]] || continue
            raw="${raw:+$raw,}$stage"
        done
    fi
    printf '%s\n' "${raw:-none}"
}

epoch_delta() {
    local start_file="$1" end_file="$2" start end
    if [[ -r "$start_file" ]]; then start="$(tr -d '[:space:]' <"$start_file")"; else start=''; fi
    if [[ -r "$end_file" ]]; then end="$(tr -d '[:space:]' <"$end_file")"; else end=''; fi
    if [[ "$start" =~ ^[0-9]+$ && "$end" =~ ^[0-9]+$ && "$end" -ge "$start" ]]; then
        printf '%s\n' "$((end - start))"
    else
        printf 'unavailable\n'
    fi
}

gh_bin="${GH_BIN:-gh}"
repo="${RELEASE_REPO:-Richards-LLC/cassy}"

epoch_of() {
    date -u -d "$1" +%s 2>/dev/null || return 1
}

published_at="$("$gh_bin" release view "$tag" --repo "$repo" --json publishedAt --jq '.publishedAt // empty')"
if [[ -z "$published_at" ]]; then
    echo "error: release $tag is not published yet" >&2
    exit 1
fi

runs_json="$("$gh_bin" api "repos/$repo/actions/workflows/release.yml/runs?branch=$tag&event=push&per_page=50")"
# The tag push itself is the clock start, so take the FIRST run created for
# this tag. A rerun or a later attempt must never be allowed to shorten the
# measured latency.
tag_pushed_at="$(jq -r '[.workflow_runs[]?.created_at] | sort | first // empty' <<<"$runs_json")"
tag_run_id="$(jq -r '[.workflow_runs[]? | {id, created_at}] | sort_by(.created_at) | first.id // empty' <<<"$runs_json")"
if [[ -z "$tag_pushed_at" ]]; then
    echo "error: no Release workflow run found for $tag; cannot time its publication" >&2
    exit 1
fi

start="$(epoch_of "$tag_pushed_at")" || {
    echo "error: could not parse tag push timestamp $tag_pushed_at" >&2
    exit 1
}
end="$(epoch_of "$published_at")" || {
    echo "error: could not parse publish timestamp $published_at" >&2
    exit 1
}
latency=$((end - start))

within=true
if [[ "$latency" -lt 0 ]]; then
    echo "error: publication ($published_at) precedes the tag push ($tag_pushed_at)" >&2
    exit 1
fi
if [[ "$latency" -gt "$budget" ]]; then
    within=false
fi

printf 'TAG=%s\n' "$tag"
printf 'TAG_RUN_ID=%s\n' "$tag_run_id"
printf 'TAG_PUSHED_AT=%s\n' "$tag_pushed_at"
printf 'PUBLISHED_AT=%s\n' "$published_at"
printf 'PUBLISH_LATENCY_SECONDS=%s\n' "$latency"
printf 'BUDGET_SECONDS=%s\n' "$budget"
printf 'WITHIN_BUDGET=%s\n' "$within"
printf 'INTERVENTIONS=%s\n' "$(intervention_count)"
printf 'BLOCKERS=%s\n' "$(intervention_blockers)"
printf 'GREEN_TO_PIPELINE_SECS=%s\n' "$(epoch_delta "${run_dir:-}/gate.green.epoch" "${run_dir:-}/pipeline.start.epoch")"
printf 'MERGED_TO_PUBLISHER_SECS=%s\n' "$(epoch_delta "${run_dir:-}/pipeline.merged.epoch" "${run_dir:-}/publisher.start.epoch")"

if ! "$within"; then
    echo "error: $tag took ${latency}s from tag push to publication, over the ${budget}s budget" >&2
    exit 1
fi

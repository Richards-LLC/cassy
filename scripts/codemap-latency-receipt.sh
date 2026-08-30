#!/usr/bin/env bash
# Measure the codemap refresh path without changing CODEMAP.md.
#
# The local phases are deliberately separate from GitHub scheduling.  A
# no-op render is a valid rehearsal: if the rendered bytes differ, this script
# fails rather than silently creating a content-changing codemap commit.
set -uo pipefail

usage() {
    cat >&2 <<'EOF'
Usage: scripts/codemap-latency-receipt.sh [options]

Options:
  --repo-root <path>       Checkout to rehearse (default: current checkout)
  --artifact <path>        Also write the key=value receipt to this path
  --rendered-path <path>   Candidate render to compare with CODEMAP.md
  --cas-bin <path>         cas executable (default: $CAS_BIN or cas)
  --github-run-id <id>     Optional Actions run for queue/required-job timing
  --github-repo <slug>     Actions repository (default: $GITHUB_REPOSITORY)
  --help                   Show this help

Budgets are configurable for controlled probes with:
  CODEMAP_AGENT_BUDGET_SECONDS (default 300)
  CODEMAP_KNOWLEDGE_BUDGET_SECONDS (default 90)
  CODEMAP_DOCS_ONLY_BUDGET_SECONDS (default 60)
EOF
}

repo_root=""
artifact=""
rendered_path=""
cas_bin="${CAS_BIN:-cas}"
github_run_id=""
github_repo="${GITHUB_REPOSITORY:-Richards-LLC/cassy}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --repo-root)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            repo_root="$2"
            shift 2
            ;;
        --artifact)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            artifact="$2"
            shift 2
            ;;
        --rendered-path)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            rendered_path="$2"
            shift 2
            ;;
        --cas-bin)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            cas_bin="$2"
            shift 2
            ;;
        --github-run-id)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            github_run_id="$2"
            shift 2
            ;;
        --github-repo)
            [[ $# -ge 2 ]] || { usage; exit 2; }
            github_repo="$2"
            shift 2
            ;;
        --help)
            usage
            exit 0
            ;;
        *)
            echo "error: unknown option: $1" >&2
            usage
            exit 2
            ;;
    esac
done

if [[ -z "$repo_root" ]]; then
    repo_root="$(git rev-parse --show-toplevel 2>/dev/null)" || {
        echo "error: run from a Git checkout or pass --repo-root" >&2
        exit 2
    }
else
    repo_root="$(cd "$repo_root" && pwd)" || exit 2
fi

if ! git -C "$repo_root" rev-parse --show-toplevel >/dev/null 2>&1; then
    echo "error: not a Git checkout: $repo_root" >&2
    exit 2
fi

codemap_path="$repo_root/.claude/CODEMAP.md"
[[ -f "$codemap_path" ]] || {
    echo "error: missing $codemap_path" >&2
    exit 2
}

if [[ -z "$rendered_path" ]]; then
    rendered_path="$codemap_path"
elif [[ "$rendered_path" != /* ]]; then
    rendered_path="$repo_root/$rendered_path"
fi
[[ -f "$rendered_path" ]] || {
    echo "error: missing render candidate $rendered_path" >&2
    exit 2
}

if [[ "$cas_bin" == */* && "$cas_bin" != /* ]]; then
    cas_bin="$repo_root/$cas_bin"
fi

agent_budget="${CODEMAP_AGENT_BUDGET_SECONDS:-300}"
knowledge_budget="${CODEMAP_KNOWLEDGE_BUDGET_SECONDS:-90}"
docs_only_budget="${CODEMAP_DOCS_ONLY_BUDGET_SECONDS:-60}"
for budget_name in agent_budget knowledge_budget docs_only_budget; do
    budget_value="${!budget_name}"
    if [[ ! "$budget_value" =~ ^[0-9]+$ ]] || (( budget_value == 0 )); then
        echo "error: ${budget_name} must be a positive integer" >&2
        exit 2
    fi
done

temp_suffix=XX
temp_suffix="${temp_suffix}XX"
temp_suffix="${temp_suffix}XX"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/codemap-latency.${temp_suffix}")" || exit 2
probe_worktree=""
cleanup() {
    if [[ -n "$probe_worktree" ]]; then
        git -C "$repo_root" worktree remove --force "$probe_worktree" >/dev/null 2>&1 || true
    fi
    rm -rf "$tmp"
}
trap cleanup EXIT

now_seconds() {
    if [[ -n "${CODEMAP_LATENCY_CLOCK:-}" ]]; then
        "${CODEMAP_LATENCY_CLOCK}"
    else
        date +%s
    fi
}

structure_seconds=0
structure_status=0
static_seconds=0
static_status=0
knowledge_seconds=0
knowledge_status=0
readiness_seconds=0
readiness_status=0
docs_local_seconds=0
docs_local_status=0

run_phase() {
    phase="$1"
    shift
    phase_start="$(now_seconds)"
    "$@" >"$tmp/$phase.log" 2>&1
    phase_status=$?
    phase_end="$(now_seconds)"
    if [[ ! "$phase_start" =~ ^[0-9]+$ || ! "$phase_end" =~ ^[0-9]+$ ]] || (( phase_end < phase_start )); then
        echo "error: clock returned invalid values for $phase" >&2
        phase_status=2
        phase_elapsed=0
    else
        phase_elapsed=$((phase_end - phase_start))
    fi
    case "$phase" in
        structure) structure_seconds="$phase_elapsed"; structure_status="$phase_status" ;;
        static) static_seconds="$phase_elapsed"; static_status="$phase_status" ;;
        knowledge) knowledge_seconds="$phase_elapsed"; knowledge_status="$phase_status" ;;
        readiness) readiness_seconds="$phase_elapsed"; readiness_status="$phase_status" ;;
        docs) docs_local_seconds="$phase_elapsed"; docs_local_status="$phase_status" ;;
        *) echo "error: unknown phase $phase" >&2; return 2 ;;
    esac
}

phase_structure() {
    git -C "$repo_root" ls-tree -d --name-only HEAD >"$tmp/top-level-directories" || return 1
    top_level_count="$(wc -l <"$tmp/top-level-directories" | tr -d '[:space:]')"
    cmp -s "$rendered_path" "$codemap_path" || {
        echo "render candidate differs from CODEMAP.md" >&2
        return 1
    }
    return 0
}

freshness_status="unknown"
phase_static() {
    (cd "$repo_root" && "$cas_bin" codemap status) || return $?
}

phase_knowledge() {
    # The CLI owns process cancellation/reaping. This wrapper only measures
    # the one bounded invocation and records its nonzero result as evidence.
    (cd "$repo_root" && "$cas_bin" knowledge build --timeout-secs "$knowledge_budget" --max-sources 5)
}

phase_readiness() {
    git -C "$repo_root" diff --check || return 1
    git -C "$repo_root" diff --cached --check || return 1
    git -C "$repo_root" rev-parse --verify HEAD >/dev/null || return 1
    git -C "$repo_root" symbolic-ref --quiet --short HEAD >/dev/null || return 1
    git -C "$repo_root" remote get-url origin >/dev/null || return 1
    test -f "$codemap_path"
}

phase_docs_only() {
    probe_worktree="$tmp/docs-only-worktree"
    git -C "$repo_root" worktree add --quiet --detach "$probe_worktree" HEAD || return 1
    mkdir -p "$probe_worktree/docs" || return 1
    printf '%s\n' '# disposable codemap latency docs probe' >"$probe_worktree/docs/.codemap-latency-probe.md" || return 1
    git -C "$probe_worktree" add docs/.codemap-latency-probe.md || return 1
    git -C "$probe_worktree" \
        -c user.name='Cassy latency probe' \
        -c user.email='cassy-latency-probe@example.invalid' \
        commit --quiet --no-verify -m 'test: disposable codemap latency docs probe' || return 1

    docs_probe_base="$(git -C "$probe_worktree" rev-parse HEAD^)" || return 1
    docs_probe_head="$(git -C "$probe_worktree" rev-parse HEAD)" || return 1
    docs_probe_class="$("$repo_root/scripts/classify-ci-diff.sh" "$docs_probe_base" "$docs_probe_head")" || return 1
    [[ "$docs_probe_class" == docs-only ]] || {
        echo "docs-only probe classified as '$docs_probe_class'" >&2
        return 1
    }

    # This is the deterministic local proxy for the required path. It checks
    # every CI-tier contract and, unlike a GitHub run, has no runner queue.
    "$repo_root/scripts/test-ci-test-tiers.sh" >/dev/null || return 1
    git -C "$repo_root" worktree remove --force "$probe_worktree" >/dev/null || return 1
    probe_worktree=""
    return 0
}

run_phase structure phase_structure
run_phase static phase_static
if grep -qF 'Status: up to date' "$tmp/static.log"; then
    freshness_status=up-to-date
elif grep -qF 'Status: stale' "$tmp/static.log"; then
    freshness_status=stale
elif grep -qF 'CODEMAP.md: not found' "$tmp/static.log"; then
    freshness_status=missing
fi
run_phase knowledge phase_knowledge
run_phase readiness phase_readiness
run_phase docs phase_docs_only

epoch_of() {
    timestamp="$1"
    if [[ "$timestamp" == *.* ]]; then
        normalized="${timestamp%%.*}Z"
    else
        normalized="$timestamp"
    fi
    date -u -d "$normalized" +%s 2>/dev/null && return 0
    date -u -j -f '%Y-%m-%dT%H:%M:%SZ' "$normalized" +%s 2>/dev/null
}

github_queue_status=not-requested
github_queue_seconds=not-requested
github_required_status=not-requested
github_required_seconds="$docs_local_seconds"
github_required_source=local-contract-proxy
github_url=not-requested
phase_log_dir=not-retained
if [[ -n "$artifact" ]]; then
    phase_log_dir="${artifact}.logs"
fi

measure_github_run() {
    gh_bin="${GH_BIN:-gh}"
    github_json="$($gh_bin run view "$github_run_id" --repo "$github_repo" --json createdAt,jobs,url 2>"$tmp/github.log")" || {
        github_queue_status=unavailable
        github_required_status=unavailable
        return 0
    }
    command -v jq >/dev/null 2>&1 || {
        github_queue_status=unavailable
        github_required_status=unavailable
        echo 'jq is required to parse GitHub run timing' >>"$tmp/github.log"
        return 0
    }

    github_url="$(jq -r '.url // "unknown"' <<<"$github_json")"
    created_at="$(jq -r '.createdAt // empty' <<<"$github_json")"
    first_started_at="$(jq -r '[.jobs[] | select(.conclusion != "skipped" and .startedAt != null) | .startedAt] | min // empty' <<<"$github_json")"
    if [[ -n "$created_at" && -n "$first_started_at" ]]; then
        created_epoch="$(epoch_of "$created_at")"
        first_started_epoch="$(epoch_of "$first_started_at")"
        if [[ "$created_epoch" =~ ^[0-9]+$ && "$first_started_epoch" =~ ^[0-9]+$ ]] && (( first_started_epoch >= created_epoch )); then
            github_queue_seconds=$((first_started_epoch - created_epoch))
            github_queue_status=measured
        else
            github_queue_status=invalid
        fi
    else
        github_queue_status=unavailable
    fi

    required_job_seconds=()
    for required_job in 'Fast Validation' 'macOS Check'; do
        job_start="$(jq -r --arg name "$required_job" '[.jobs[] | select(.name == $name and .startedAt != null and .completedAt != null) | .startedAt][0] // empty' <<<"$github_json")"
        job_end="$(jq -r --arg name "$required_job" '[.jobs[] | select(.name == $name and .startedAt != null and .completedAt != null) | .completedAt][0] // empty' <<<"$github_json")"
        if [[ -z "$job_start" || -z "$job_end" ]]; then
            github_required_status=unavailable
            return 0
        fi
        job_start_epoch="$(epoch_of "$job_start")"
        job_end_epoch="$(epoch_of "$job_end")"
        if [[ ! "$job_start_epoch" =~ ^[0-9]+$ || ! "$job_end_epoch" =~ ^[0-9]+$ ]] || (( job_end_epoch < job_start_epoch )); then
            github_required_status=invalid
            return 0
        fi
        required_job_seconds[${#required_job_seconds[@]}]=$((job_end_epoch - job_start_epoch))
    done
    github_required_seconds="${required_job_seconds[0]}"
    if (( required_job_seconds[1] > github_required_seconds )); then
        github_required_seconds="${required_job_seconds[1]}"
    fi
    github_required_status=measured
    github_required_source=github-required-jobs
}

if [[ -n "$github_run_id" ]]; then
    measure_github_run
fi

agent_total=$((structure_seconds + static_seconds + knowledge_seconds + readiness_seconds))
agent_within_budget=true
knowledge_within_budget=true
docs_only_within_budget=true
no_content_change=true
if [[ "$structure_status" -ne 0 ]]; then no_content_change=false; fi
if (( agent_total > agent_budget )); then agent_within_budget=false; fi
if (( knowledge_seconds > knowledge_budget )); then knowledge_within_budget=false; fi
if (( github_required_seconds > docs_only_budget )); then docs_only_within_budget=false; fi

head_sha="$(git -C "$repo_root" rev-parse HEAD 2>/dev/null || echo unknown)"
generated_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
receipt="$tmp/receipt.env"
{
    printf 'RECEIPT_VERSION=1\n'
    printf 'GENERATED_AT_UTC=%s\n' "$generated_at"
    printf 'REPO_HEAD=%s\n' "$head_sha"
    printf 'REPO_ROOT=%s\n' "$repo_root"
    printf 'CODEMAP_RENDER_STATUS=%s\n' "$([[ "$no_content_change" == true ]] && echo identical || echo changed)"
    printf 'NO_CONTENT_CHANGE_PRESERVED=%s\n' "$no_content_change"
    printf 'TOP_LEVEL_DIRECTORY_COUNT=%s\n' "${top_level_count:-unknown}"
    printf 'STRUCTURE_SCAN_RENDER_SECONDS=%s\n' "$structure_seconds"
    printf 'STRUCTURE_SCAN_RENDER_EXIT_STATUS=%s\n' "$structure_status"
    printf 'STATIC_FRESHNESS_PROOF_SECONDS=%s\n' "$static_seconds"
    printf 'STATIC_FRESHNESS_PROOF_EXIT_STATUS=%s\n' "$static_status"
    printf 'CODEMAP_FRESHNESS_STATUS=%s\n' "$freshness_status"
    printf 'KNOWLEDGE_BUILD_SECONDS=%s\n' "$knowledge_seconds"
    printf 'KNOWLEDGE_BUILD_EXIT_STATUS=%s\n' "$knowledge_status"
    printf 'KNOWLEDGE_BUILD_BUDGET_SECONDS=%s\n' "$knowledge_budget"
    printf 'KNOWLEDGE_BUILD_WITHIN_BUDGET=%s\n' "$knowledge_within_budget"
    printf 'LOCAL_COMMIT_PUSH_READINESS_SECONDS=%s\n' "$readiness_seconds"
    printf 'LOCAL_COMMIT_PUSH_READINESS_EXIT_STATUS=%s\n' "$readiness_status"
    printf 'AGENT_CONTROLLED_TOTAL_SECONDS=%s\n' "$agent_total"
    printf 'AGENT_CONTROLLED_BUDGET_SECONDS=%s\n' "$agent_budget"
    printf 'AGENT_CONTROLLED_WITHIN_BUDGET=%s\n' "$agent_within_budget"
    printf 'DOCS_ONLY_LOCAL_CONTRACT_SECONDS=%s\n' "$docs_local_seconds"
    printf 'DOCS_ONLY_LOCAL_CONTRACT_EXIT_STATUS=%s\n' "$docs_local_status"
    printf 'DOCS_ONLY_REQUIRED_COMPUTE_SECONDS=%s\n' "$github_required_seconds"
    printf 'DOCS_ONLY_REQUIRED_COMPUTE_SOURCE=%s\n' "$github_required_source"
    printf 'DOCS_ONLY_REQUIRED_COMPUTE_BUDGET_SECONDS=%s\n' "$docs_only_budget"
    printf 'DOCS_ONLY_REQUIRED_COMPUTE_WITHIN_BUDGET=%s\n' "$docs_only_within_budget"
    printf 'GITHUB_RUN_ID=%s\n' "${github_run_id:-not-requested}"
    printf 'GITHUB_RUN_URL=%s\n' "$github_url"
    printf 'GITHUB_QUEUE_SECONDS=%s\n' "$github_queue_seconds"
    printf 'GITHUB_QUEUE_STATUS=%s\n' "$github_queue_status"
    printf 'GITHUB_REQUIRED_JOBS_STATUS=%s\n' "$github_required_status"
    printf 'PHASE_LOG_DIR=%s\n' "$phase_log_dir"
} >"$receipt"

cat "$receipt"
if [[ -n "$artifact" ]]; then
    mkdir -p "$(dirname "$artifact")"
    cp "$receipt" "$artifact"
    mkdir -p "$phase_log_dir"
    cp "$tmp"/*.log "$phase_log_dir/" 2>/dev/null || true
    echo "ARTIFACT_PATH=$artifact"
fi

if [[ "$no_content_change" != true \
    || "$static_status" -ne 0 \
    || "$readiness_status" -ne 0 \
    || "$docs_local_status" -ne 0 \
    || "$agent_within_budget" != true \
    || "$knowledge_within_budget" != true \
    || "$docs_only_within_budget" != true ]]; then
    echo "error: codemap latency budget or no-content invariant failed" >&2
    for phase in structure static knowledge readiness docs; do
        phase_status_file="$tmp/$phase.log"
        case "$phase" in
            structure) phase_result="$structure_status" ;;
            static) phase_result="$static_status" ;;
            knowledge) phase_result="$knowledge_status" ;;
            readiness) phase_result="$readiness_status" ;;
            docs) phase_result="$docs_local_status" ;;
        esac
        if [[ "$phase_result" -ne 0 ]]; then
            echo "--- $phase output (exit $phase_result) ---" >&2
            tail -n 5 "$phase_status_file" >&2 || true
        fi
    done
    exit 1
fi

# A knowledge provider failure is best-effort per the codemap skill. Its exit
# status is retained in the receipt; only the measured wall-clock bound gates
# this command.
exit 0

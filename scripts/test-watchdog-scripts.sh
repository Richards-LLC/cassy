#!/usr/bin/env bash
# Behavioral fixtures for both GitHub Actions cancellation watchdogs.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
merge_script="$repo_root/scripts/cancel-stale-merge-group-runs.sh"
queued_script="$repo_root/scripts/cancel-stale-non-merge-group-queued-runs.sh"
pass=0
fail=0

expect_text() {
    local haystack="$1" needle="$2" label="$3"
    if grep -qF -- "$needle" <<<"$haystack"; then
        printf 'ok   %s\n' "$label"; pass=$((pass + 1))
    else
        printf 'FAIL %s (missing %s)\n' "$label" "$needle" >&2; fail=$((fail + 1))
    fi
}

expect_absent() {
    local haystack="$1" needle="$2" label="$3"
    if grep -qF -- "$needle" <<<"$haystack"; then
        printf 'FAIL %s (unexpected %s)\n' "$label" "$needle" >&2; fail=$((fail + 1))
    else
        printf 'ok   %s\n' "$label"; pass=$((pass + 1))
    fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"
cat >"$tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${FAKE_GH_LOG:?}"
case "$*" in
  *'event=merge_group&status=in_progress&per_page=100'*)
    printf '%s\n' '{"workflow_runs":[{"id":201,"name":"CI","run_started_at":"1970-01-01T00:00:00Z","created_at":"1970-01-01T00:00:00Z","head_branch":"queue","status":"in_progress"},{"id":202,"name":"CI","run_started_at":"1970-01-01T00:31:00Z","created_at":"1970-01-01T00:31:00Z","head_branch":"fresh","status":"in_progress"},{"id":203,"name":"Other workflow","run_started_at":"1970-01-01T00:00:00Z","created_at":"1970-01-01T00:00:00Z","head_branch":"other","status":"in_progress"}]}' ;;
  *'event=merge_group&status=queued&per_page=100'*)
    printf '%s\n' '{"workflow_runs":[{"id":204,"name":"CI","run_started_at":null,"created_at":"1970-01-01T00:00:00Z","head_branch":"queued","status":"queued"},{"id":205,"name":"CI","run_started_at":null,"created_at":"1970-01-01T00:00:00Z","head_branch":"raced","status":"queued"}]}' ;;
  *'actions/runs?status=queued&per_page=100'*)
    printf '%s\n' '{"workflow_runs":[{"id":101,"created_at":"1970-01-01T00:00:00Z","event":"push","head_branch":"main"},{"id":102,"created_at":"1970-01-01T00:00:00Z","event":"merge_group","head_branch":"queue"},{"id":103,"created_at":"1970-01-01T00:31:00Z","event":"pull_request","head_branch":"fresh"},{"id":104,"created_at":"1970-01-01T00:00:00Z","event":"workflow_dispatch","head_branch":"raced"},{"id":105,"created_at":"not-a-date","event":"push","head_branch":"invalid"}]}' ;;
  *'actions/runs/201/jobs?filter=latest&per_page=100'*|*'actions/runs/204/jobs?filter=latest&per_page=100'*)
    printf '%s\n' '{"jobs":[{"name":"Fast Validation — suite archive build","status":"in_progress","started_at":"1970-01-01T00:00:00Z","created_at":"1970-01-01T00:00:00Z"}]}' ;;
  *'actions/runs/202/jobs?filter=latest&per_page=100'*)
    printf '%s\n' '{"jobs":[{"name":"Fast Validation — suite archive build","status":"in_progress","started_at":"1970-01-01T00:31:00Z","created_at":"1970-01-01T00:31:00Z"}]}' ;;
  *'actions/runs/205/jobs?filter=latest&per_page=100'*)
    count_file="${FAKE_GH_STATE:?}/205-count"; count=0; [[ -f "$count_file" ]] && count="$(<"$count_file")"; echo $((count + 1)) >"$count_file"
    if (( count == 0 )); then status=queued; else status=completed; fi
    printf '{"jobs":[{"name":"Fast Validation — suite archive build","status":"%s","started_at":null,"created_at":"1970-01-01T00:00:00Z"}]}' "$status" ;;
  *'actions/runs/201 --jq .status'*) printf 'in_progress\n' ;;
  *'actions/runs/204 --jq .status'*) printf 'queued\n' ;;
  *'actions/runs/205 --jq .status'*) printf 'queued\n' ;;
  *'actions/runs/101 --jq .status'*) printf 'queued\n' ;;
  *'actions/runs/104 --jq .status'*) printf 'completed\n' ;;
  *'--method POST'*'actions/runs/'*) exit 0 ;;
  *) printf 'unexpected fake gh invocation: %s\n' "$*" >&2; exit 2 ;;
esac
EOF
chmod +x "$tmp/bin/gh"

run_env=(GITHUB_REPOSITORY=example/repo CASSY_NOW_EPOCH=2000 FAKE_GH_LOG="$tmp/gh.log" FAKE_GH_STATE="$tmp/state" PATH="$tmp/bin:$PATH")
mkdir -p "$tmp/state"
merge_output="$tmp/merge-output"
if env "${run_env[@]}" "$merge_script" >"$merge_output" 2>&1; then
    merge_text="$(<"$merge_output")"; gh_log="$(<"$tmp/gh.log")"
    expect_text "$merge_text" 'cancelling stale merge_group run=201' 'merge watchdog cancels stale in-progress CI archive job'
    expect_text "$merge_text" 'cancelling stale merge_group run=204' 'merge watchdog falls back to created_at for queued archive job'
    expect_text "$merge_text" 'skipping inactive archive job for run=205' 'merge watchdog tolerates a job completing mid-sweep'
    expect_absent "$gh_log" 'actions/runs/202/cancel' 'merge watchdog preserves fresh archive job'
    expect_absent "$gh_log" 'actions/runs/203/jobs' 'merge watchdog ignores non-CI merge-group workflows'
    expect_absent "$gh_log" 'actions/runs/205/cancel' 'merge watchdog does not cancel a completed job'
else
    printf 'FAIL merge watchdog fixture exits successfully\n' >&2
    cat "$merge_output" >&2
    fail=$((fail + 1))
fi

: >"$tmp/gh.log"
queued_output="$tmp/queued-output"
if env "${run_env[@]}" "$queued_script" >"$queued_output" 2>&1; then
    queued_text="$(<"$queued_output")"; gh_log="$(<"$tmp/gh.log")"
    expect_text "$queued_text" 'cancelling stale queued run=101 event=push' 'queued watchdog cancels stale non-merge-group run'
    expect_text "$queued_text" 'skipping no-longer-queued run=104 current_status=completed' 'queued watchdog tolerates a run completing mid-sweep'
    expect_text "$queued_text" 'skipping queued run=105 with invalid created_at=not-a-date' 'queued watchdog guards invalid dates'
    expect_absent "$gh_log" 'actions/runs/102' 'queued watchdog leaves merge-group scope alone'
    expect_absent "$gh_log" 'actions/runs/103' 'queued watchdog preserves threshold-fresh run'
    expect_text "$gh_log" 'actions/runs/101/cancel' 'queued watchdog sends cancel only after status re-read'
else
    printf 'FAIL queued watchdog fixture exits successfully\n' >&2; fail=$((fail + 1))
fi

for script in "$merge_script" "$queued_script"; do
    dry_output="$tmp/$(basename "$script").dry-output"
    if env "${run_env[@]}" CASSY_WATCHDOG_DRY_RUN=true "$script" >"$dry_output" 2>&1; then
        if [[ "$script" == "$merge_script" ]]; then run_id=201; else run_id=101; fi
        expect_text "$(<"$dry_output")" "dry-run would cancel run=$run_id" "$(basename "$script") dry-run reports without posting"
    else
        printf 'FAIL %s dry-run fixture exits successfully\n' "$(basename "$script")" >&2; fail=$((fail + 1))
    fi
done

for script in "$merge_script" "$queued_script"; do
    if env -u GITHUB_REPOSITORY CASSY_NOW_EPOCH=2000 "$script" >/dev/null 2>&1; then
        printf 'FAIL %s rejects missing repository\n' "$(basename "$script")" >&2; fail=$((fail + 1))
    elif [[ $? -eq 2 ]]; then
        printf 'ok   %s requires an explicit repository\n' "$(basename "$script")"; pass=$((pass + 1))
    else
        printf 'FAIL %s missing repository exits 2\n' "$(basename "$script")" >&2; fail=$((fail + 1))
    fi
    if env GITHUB_REPOSITORY=example/repo CASSY_WATCHDOG_STALE_SECONDS=invalid "$script" >/dev/null 2>&1; then
        printf 'FAIL %s rejects invalid threshold\n' "$(basename "$script")" >&2; fail=$((fail + 1))
    elif [[ $? -eq 2 ]]; then
        printf 'ok   %s invalid threshold exits 2\n' "$(basename "$script")"; pass=$((pass + 1))
    else
        printf 'FAIL %s invalid threshold exits 2\n' "$(basename "$script")" >&2; fail=$((fail + 1))
    fi
done

printf 'test result: %s passed; %s failed\n' "$pass" "$fail"
(( fail == 0 ))

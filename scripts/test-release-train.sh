#!/usr/bin/env bash
# Fixture-driven self-test for scripts/release-train.sh (cas-5212).
#
# The release train's contract here is about IDENTITY, not about building a
# release: two supervisors gating the same version from different epic
# worktrees must not share a run directory, must not be able to signal each
# other, and must never be located by a process-name pattern. The gate itself
# is stubbed — spending a release's build time inside this self-test would
# prove nothing about that contract.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
train="$script_dir/release-train.sh"
repo_root="$(cd "$script_dir/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

pass=0
fail=0
ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }

# Every fixture keeps its artifacts under the test's own temp dir: this suite
# must never write to the operator's ~/.cas.
export CAS_RELEASE_ARTIFACTS_ROOT="$tmp/artifacts"

new_worktree() {
    local name="$1"
    local dir="$tmp/$name"
    mkdir -p "$dir"
    ( cd "$dir"
      git init -q -b main .
      git config user.email test@test.invalid
      git config user.name 'Release Train Test'
      echo seed > seed.txt
      mkdir -p scripts cas-cli/src/builtins
      : > cas-cli/src/builtins/reference-history.json
      cat > scripts/gen-builtin-reference-history.sh <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${TRAIN_FIXTURE_LEDGER_DIRTY:-}" == 1 ]]; then
  printf 'changed ledger\n' > cas-cli/src/builtins/reference-history.json
fi
EOF
      chmod +x scripts/gen-builtin-reference-history.sh
      git add seed.txt
      git add scripts cas-cli
      git -c commit.gpgsign=false commit -q -m seed ) >/dev/null
    printf '%s\n' "$dir"
}

# A stub standing in for release-gate.sh: it records that it ran and how, then
# exits with the status the fixture asked for.
new_gate_stub() {
    local path="$1" exit_code="$2" sleep_for="${3:-0}"
    cat >"$path" <<EOF
#!/usr/bin/env bash
printf 'stub gate version=%s cwd=%s args=%s\n' "\$1" "\$PWD" "\$*"
if [[ $sleep_for -gt 0 ]]; then
  sleep $sleep_for &
  child=\$!
  [[ -z "\${GATE_STUB_CHILD_PID_FILE:-}" ]] || printf '%s\n' "\$child" >"\$GATE_STUB_CHILD_PID_FILE"
  wait "\$child"
fi
exit $exit_code
EOF
    chmod +x "$path"
}

wait_gate_done() {
    local run_dir="$1"
    for _ in $(seq 1 100); do
        [[ -s "$run_dir/gate.done" ]] && return 0
        sleep 0.05
    done
    return 1
}

wait_for_file() {
    local path_pattern="$1"
    for _ in $(seq 1 100); do
        compgen -G "$path_pattern" >/dev/null && return 0
        sleep 0.05
    done
    return 1
}

# ---------------------------------------------------------------------------
# The run directory is keyed by the worktree, not by the version alone.
# ---------------------------------------------------------------------------
wt_a="$(new_worktree epic-a-merge)"
wt_b="$(new_worktree epic-b-merge)"

dir_a="$("$train" 9.99.0 "$wt_a" --print-run-dir)"
dir_b="$("$train" 9.99.0 "$wt_b" --print-run-dir)"

if [[ "$dir_a" != "$dir_b" ]]; then
    ok 'two worktrees at the same version resolve to different run directories'
else
    bad "both worktrees resolved to $dir_a"
fi
if [[ "$dir_a" == *"9.99.0"* && "$dir_a" == *"epic-a-merge"* ]]; then
    ok 'the run directory names both the version and the worktree'
else
    bad "run directory does not identify the run: $dir_a"
fi

# ---------------------------------------------------------------------------
# A completed gate leaves an attributable receipt.
# ---------------------------------------------------------------------------
gate_ok="$tmp/gate-ok.sh"
new_gate_stub "$gate_ok" 0
CAS_RELEASE_TRAIN_GATE_CMD="$gate_ok" "$train" 9.99.0 "$wt_a" --gate >/dev/null 2>&1 || true
wait_gate_done "$dir_a" || true

if [[ "$(cat "$dir_a/gate.done" 2>/dev/null)" == "0" ]]; then
    ok 'a successful gate records its exit status in gate.done'
else
    bad "gate.done missing or non-zero: $(cat "$dir_a/gate.done" 2>/dev/null || echo absent)"
fi
if [[ "$(cat "$dir_a/gate.full.sha" 2>/dev/null)" == "$(git -C "$wt_a" rev-parse HEAD)" ]]; then
    ok 'a successful full gate records the exact commit it proved'
else
    bad "full-gate commit receipt missing or stale: $(cat "$dir_a/gate.full.sha" 2>/dev/null || echo absent)"
fi
if grep -q "$wt_a" "$dir_a/run.env" 2>/dev/null; then
    ok 'run.env attributes the run to its worktree'
else
    bad "run.env does not name the worktree: $(cat "$dir_a/run.env" 2>/dev/null || echo absent)"
fi
if grep -q "stub gate version=9.99.0" "$dir_a/gate.log" 2>/dev/null; then
    ok 'the gate log lands in the run directory'
else
    bad "gate.log missing or empty: $(cat "$dir_a/gate.log" 2>/dev/null || echo absent)"
fi

# ---------------------------------------------------------------------------
# cas-c0411. The gate raises the `cas init` watchdog budget for its children by
# exporting CAS_INIT_TIMEOUT_SECS, and everything that matters — the tests that
# spawn `cas init` — sits below the gate, so the train must hand the gate an
# environment rather than a sanitized one. `env -i`, or a nohup wrapper that
# rebuilt the environment, would put those children back on the 300s default
# that failed the v3.15.1 archive-mode row, and nothing else here would notice.
# ---------------------------------------------------------------------------
gate_env="$tmp/gate-env.sh"
cat >"$gate_env" <<'EOF'
#!/usr/bin/env bash
printf 'stub gate version=%s cwd=%s\n' "$1" "$PWD"
printf 'CAS_INIT_TIMEOUT_SECS=%s\n' "${CAS_INIT_TIMEOUT_SECS:-unset}"
printf 'CAS_RELEASE_GATE_HOME_DIR=%s\n' "${CAS_RELEASE_GATE_HOME_DIR:-unset}"
EOF
chmod +x "$gate_env"
wt_env="$(new_worktree epic-env-merge)"
dir_env="$("$train" 9.99.0 "$wt_env" --print-run-dir)"
CAS_RELEASE_TRAIN_GATE_CMD="$gate_env" CAS_INIT_TIMEOUT_SECS=900 \
    "$train" 9.99.0 "$wt_env" --gate >/dev/null 2>&1 || true
wait_gate_done "$dir_env" || true

if grep -qx 'CAS_INIT_TIMEOUT_SECS=900' "$dir_env/gate.log" 2>/dev/null; then
    ok 'the train hands the gate its environment, so the raised init budget survives'
else
    bad "the train did not forward CAS_INIT_TIMEOUT_SECS to the gate: $(cat "$dir_env/gate.log" 2>/dev/null || echo absent)"
fi
if grep -q '^CAS_RELEASE_GATE_HOME_DIR=/' "$dir_env/gate.log" 2>/dev/null; then
    ok 'the scratch base the train sets reaches the gate in the same environment'
else
    bad "the gate ran without a scratch base: $(cat "$dir_env/gate.log" 2>/dev/null || echo absent)"
fi

# The ledger is regenerated synchronously, after every merge/learn opportunity
# and before any detached process starts.
wt_ledger="$(new_worktree epic-ledger-merge)"
dir_ledger="$("$train" 9.99.1 "$wt_ledger" --print-run-dir)"
out="$(TRAIN_FIXTURE_LEDGER_DIRTY=1 CAS_RELEASE_TRAIN_GATE_CMD="$gate_ok" \
    "$train" 9.99.1 "$wt_ledger" --gate 2>&1 || true)"
if [[ "$out" == *'commit the ledger before starting the detached gate'* ]] \
    && [[ ! -e "$dir_ledger/gate.pid" ]]; then
    ok 'ledger regeneration refuses with the commit-ledger message before detach'
else
    bad "ledger drift did not refuse before detach: $out"
fi

# A targeted rerun forwards only known non-empty rows to the gate, writes a
# diagnostic receipt, and never overwrites the full-gate authorization/history.
wt_only="$(new_worktree epic-only-merge)"
dir_only="$("$train" 9.99.2 "$wt_only" --print-run-dir)"
mkdir -p "$dir_only"
printf 'FULL GATE LOG SENTINEL\n' >"$dir_only/gate.log"
printf '0\n' >"$dir_only/gate.done"
printf '123\n' >"$dir_only/gate.green.epoch"
git -C "$wt_only" rev-parse HEAD >"$dir_only/gate.full.sha"
CAS_RELEASE_TRAIN_GATE_CMD="$gate_ok" "$train" 9.99.2 "$wt_only" \
    --gate --only nextest,doctests >/dev/null 2>&1
wait_for_file "$dir_only/diagnostics/*/gate.done" || true
diagnostic_log="$(find "$dir_only/diagnostics" -name gate.log -type f -print -quit 2>/dev/null || true)"
if [[ -n "$diagnostic_log" ]] \
    && grep -qF 'args=9.99.2 --only nextest,doctests' "$diagnostic_log"; then
    ok '--gate --only forwards selected rows to a diagnostic receipt'
else
    bad "--only diagnostic log missing or wrong: ${diagnostic_log:-absent}"
fi
if [[ "$(cat "$dir_only/gate.log")" == 'FULL GATE LOG SENTINEL' ]] \
    && [[ "$(cat "$dir_only/gate.done")" == 0 ]] \
    && [[ "$(cat "$dir_only/gate.green.epoch")" == 123 ]] \
    && [[ "$(cat "$dir_only/gate.full.sha")" == "$(git -C "$wt_only" rev-parse HEAD)" ]]; then
    ok '--gate --only preserves the prior full-gate receipt and history'
else
    bad '--gate --only overwrote a full-gate authorization receipt'
fi
for invalid in '' not-a-row; do
    wt_invalid="$(new_worktree "epic-only-invalid-${invalid:-empty}")"
    out="$(CAS_RELEASE_TRAIN_GATE_CMD="$gate_ok" "$train" 9.99.3 "$wt_invalid" \
        --gate --only "$invalid" 2>&1 || true)"
    if grep -qE 'non-empty|unknown --only' <<<"$out"; then
        ok "release-train --only rejects ${invalid:-an empty row list} before detach"
    else
        bad "release-train --only accepted '$invalid': $out"
    fi
done

# --check-lane binds the branch name and exact tip to the Scoped Validation JOB
# inside the real CI workflow's push run. Missing evidence and API errors refuse.
wt_lane="$(new_worktree lane-ci)"
remote_lane_sha="$(git -C "$wt_lane" rev-parse HEAD)"
printf 'new local tip\n' >"$wt_lane/lane-change.txt"
git -C "$wt_lane" add lane-change.txt
git -C "$wt_lane" -c commit.gpgsign=false commit -q -m 'new local lane tip'
lane_sha="$(git -C "$wt_lane" rev-parse HEAD)"
git -C "$wt_lane" update-ref refs/remotes/origin/main "$remote_lane_sha"
lane_calls="$tmp/lane-gh.calls"
lane_runs="$tmp/lane-gh.json"
lane_jobs="$tmp/lane-gh-jobs.json"
cat >"$tmp/lane-gh.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$LANE_GH_CALLS"
case "$1 $2" in
"run list")
    [[ "${LANE_GH_FAIL:-}" != list ]] || { printf 'run list API failed\n' >&2; exit 1; }
    cat "$LANE_GH_RUNS"
    ;;
"run view")
    [[ "${LANE_GH_FAIL:-}" != view ]] || { printf 'run view API failed\n' >&2; exit 1; }
    cat "$LANE_GH_JOBS"
    ;;
*) exit 2;;
esac
EOF
chmod +x "$tmp/lane-gh.sh"
run_lane_check() {
    LANE_GH_CALLS="$lane_calls" LANE_GH_RUNS="$lane_runs" LANE_GH_JOBS="$lane_jobs" \
    CAS_RELEASE_TRAIN_GH="$tmp/lane-gh.sh" "$train" 9.99.4 "$wt_lane" --check-lane main 2>&1
}
printf '[]\n' >"$lane_runs"
printf '{"jobs":[]}\n' >"$lane_jobs"
out="$(run_lane_check || true)"
[[ "$out" == *MISSING* ]] && ok '--check-lane distinguishes a missing run' \
    || bad "missing lane run was not refused: $out"
out="$(LANE_GH_FAIL=list run_lane_check || true)"
[[ "$out" == *'API ERROR'* ]] && ok '--check-lane distinguishes a run-list API error' \
    || bad "run-list API error was collapsed into missing evidence: $out"
printf '[{"databaseId":41,"headBranch":"main","headSha":"%s","status":"in_progress","conclusion":null,"event":"push","workflowName":"CI"}]\n' "$lane_sha" >"$lane_runs"
out="$(LANE_GH_FAIL=view run_lane_check || true)"
[[ "$out" == *'API ERROR'* ]] && ok '--check-lane distinguishes a run-view API error' \
    || bad "run-view API error was collapsed into missing evidence: $out"
out="$(run_lane_check || true)"
[[ "$out" == *MISSING* ]] && ok '--check-lane distinguishes a missing Scoped Validation job' \
    || bad "missing Scoped Validation job was not refused: $out"
printf '{"jobs":[{"databaseId":101,"name":"Scoped Validation (factory/PR)","status":"in_progress","conclusion":null}]}\n' >"$lane_jobs"
out="$(run_lane_check || true)"
[[ "$out" == *PENDING* ]] && ok '--check-lane distinguishes a pending run' \
    || bad "pending lane run was not refused: $out"
printf '{"jobs":[{"databaseId":102,"name":"Scoped Validation (factory/PR)","status":"completed","conclusion":"failure"}]}\n' >"$lane_jobs"
out="$(run_lane_check || true)"
[[ "$out" == *'RED (failure)'* ]] && ok '--check-lane distinguishes a red run' \
    || bad "red lane run was not refused: $out"
printf '{"jobs":[{"databaseId":103,"name":"Scoped Validation (factory/PR)","status":"completed","conclusion":"skipped"}]}\n' >"$lane_jobs"
out="$(run_lane_check || true)"
[[ "$out" == *'RED (skipped)'* ]] && ok '--check-lane never accepts a skipped push row' \
    || bad "skipped lane run was accepted: $out"
printf '[{"databaseId":40,"headBranch":"other","headSha":"%s","status":"completed","conclusion":"success","event":"push","workflowName":"CI"},{"databaseId":41,"headBranch":"main","headSha":"%s","status":"completed","conclusion":"success","event":"push","workflowName":"CI"}]\n' "$lane_sha" "$lane_sha" >"$lane_runs"
printf '{"jobs":[{"databaseId":104,"name":"Fast Validation","status":"completed","conclusion":"skipped"},{"databaseId":105,"name":"Scoped Validation (factory/PR)","status":"completed","conclusion":"success"}]}\n' >"$lane_jobs"
out="$(run_lane_check)"
[[ "$out" == *GREEN* ]] && ok '--check-lane accepts the branch tip own green run' \
    || bad "green lane run was refused: $out"
if grep -q -- '--workflow ci.yml --branch main --event push' "$lane_calls" \
    && grep -q -- 'run view 41 .*--json jobs' "$lane_calls"; then
    ok '--check-lane scopes the CI run to branch push and inspects its jobs'
else
    bad "--check-lane did not query the real workflow/job shape: $(cat "$lane_calls")"
fi

# ---------------------------------------------------------------------------
# A second start refuses while the first run's recorded pid is alive, and says
# whose run it is and what to do about it.
# ---------------------------------------------------------------------------
gate_slow="$tmp/gate-slow.sh"
new_gate_stub "$gate_slow" 0 30
start_epoch="$(date +%s)"
GATE_STUB_CHILD_PID_FILE="$tmp/child-b.pid" CAS_RELEASE_TRAIN_GATE_CMD="$gate_slow" \
    "$train" 9.99.0 "$wt_b" --gate >/dev/null 2>&1
runner_b=''
if (( $(date +%s) - start_epoch < 5 )); then
    ok '--gate returns after launching a detached gate'
else
    bad '--gate blocked instead of returning after detach'
fi
for _ in $(seq 1 50); do
    [[ -f "$dir_b/gate.pid" ]] && break
    sleep 0.1
done
held_pid="$(cat "$dir_b/gate.pid" 2>/dev/null || true)"

refusal="$(CAS_RELEASE_TRAIN_GATE_CMD="$gate_ok" "$train" 9.99.0 "$wt_b" --gate 2>&1 || true)"
if [[ "$refusal" == *"already"* || "$refusal" == *"in progress"* ]]; then
    ok 'a second gate for the same worktree refuses while the first is live'
else
    bad "second start did not refuse: $refusal"
fi
if [[ "$refusal" == *"$wt_b"* && "$refusal" == *"--status"* && "$refusal" == *"--stop"* ]]; then
    ok 'the refusal names the owning worktree and the remedy'
else
    bad "refusal does not name the owner and remedy: $refusal"
fi

# ---------------------------------------------------------------------------
# Stopping one run leaves a sibling run untouched, and only ever signals a pid
# the train itself recorded.
# ---------------------------------------------------------------------------
# The earlier successful run left its (dead) pid file behind, so wait for a
# pid that is actually alive rather than for the file to exist.
GATE_STUB_CHILD_PID_FILE="$tmp/child-a.pid" CAS_RELEASE_TRAIN_GATE_CMD="$gate_slow" \
    "$train" 9.99.0 "$wt_a" --gate >/dev/null 2>&1
runner_a=''
sibling_pid=""
for _ in $(seq 1 50); do
    candidate="$(cat "$dir_a/gate.pid" 2>/dev/null || true)"
    if [[ -n "$candidate" ]] && kill -0 "$candidate" 2>/dev/null; then
        sibling_pid="$candidate"
        break
    fi
    sleep 0.1
done

"$train" 9.99.0 "$wt_b" --stop >/dev/null 2>&1 || true
sleep 0.5

if [[ -n "$held_pid" ]] && ! kill -0 "$held_pid" 2>/dev/null; then
    ok '--stop terminates the run it recorded'
else
    bad "--stop did not terminate its own gate (pid $held_pid)"
fi
held_child="$(cat "$tmp/child-b.pid" 2>/dev/null || true)"
if [[ -n "$held_child" ]] && ! kill -0 "$held_child" 2>/dev/null; then
    ok '--stop terminates the recorded gate process group children'
else
    bad "--stop left its gate child alive (pid ${held_child:-missing})"
fi
if [[ -n "$sibling_pid" ]] && kill -0 "$sibling_pid" 2>/dev/null; then
    ok 'a concurrent run for another worktree survives its sibling being stopped'
else
    bad "the sibling run (pid $sibling_pid) died with its sibling"
fi

"$train" 9.99.0 "$wt_a" --stop >/dev/null 2>&1 || true
[[ -z "$runner_a" ]] || wait "$runner_a" 2>/dev/null || true
[[ -z "$runner_b" ]] || wait "$runner_b" 2>/dev/null || true

status="$("$train" 9.99.0 "$wt_a" --status 2>&1 || true)"
if [[ "$status" == *"$dir_a"* ]]; then
    ok '--status reports the run directory it is talking about'
else
    bad "--status did not identify the run: $status"
fi

printf 'FAIL nextest — fixture\nFAIL archive-mode — fixture\n' >"$dir_a/gate.log"
printf '100\n' >"$dir_a/gate.green.epoch"
printf '120\n' >"$dir_a/release.tag-complete.epoch"
git -C "$wt_a" tag -a -f v9.99.0 -m 'fixture release tag' HEAD
git -C "$wt_a" rev-parse HEAD >"$dir_a/landed-main.sha"
status="$("$train" 9.99.0 "$wt_a" --status 2>&1 || true)"
if [[ "$status" == *'rows_failed=nextest,archive-mode'* ]] \
    && [[ "$status" == *'cause_class=<product|fixture|environment|procedure>'* ]] \
    && [[ "$status" == *'blocking_step=<step>'* ]]; then
    ok '--status prints the required per-run epic-note template'
else
    bad "--status omitted timeline fields: $status"
fi
if [[ "$status" == *'tag publisher: completed'* ]] \
    && [[ "$status" == *'publication: pending'* ]] \
    && [[ "$status" != *'green-to-published latency:'* ]]; then
    ok '--status keeps delayed GitHub publication pending after tag success'
else
    bad "tag completion was mislabeled as publication: $status"
fi

cat >"$dir_a/release-workflow.json" <<EOF
{"headBranch":"v9.99.0","headSha":"$(git -C "$wt_a" rev-parse HEAD)","status":"completed","conclusion":"failure"}
EOF
status="$("$train" 9.99.0 "$wt_a" --status 2>&1 || true)"
if [[ "$status" == *'publication: unavailable'* ]] \
    && [[ "$status" == *'workflow conclusion=failure'* ]] \
    && [[ "$status" != *'green-to-published latency:'* ]]; then
    ok '--status never treats tag success plus release-workflow failure as published'
else
    bad "failed release workflow was mislabeled as publication: $status"
fi

cat >"$dir_a/release-workflow.json" <<EOF
{"headBranch":"v9.99.0","headSha":"$(git -C "$wt_a" rev-parse HEAD)","status":"completed","conclusion":"success"}
EOF
cat >"$dir_a/release-published.receipt" <<'EOF'
TAG=v9.99.0
PUBLISHED_AT=1970-01-01T00:02:25Z
LINUX_SHA256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
MACOS_SHA256=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
EOF
cat >"$dir_a/release-latency.receipt" <<'EOF'
TAG=v9.99.0
PUBLISHED_AT=1970-01-01T00:02:25Z
PUBLISH_LATENCY_SECONDS=25
EOF
status="$("$train" 9.99.0 "$wt_a" --status 2>&1 || true)"
if [[ "$status" == *'publication: verified at 1970-01-01T00:02:25Z'* ]] \
    && [[ "$status" == *'tag-to-published latency: 25s'* ]] \
    && [[ "$status" == *'green-to-published latency: 45s'* ]]; then
    ok '--status derives actual publication latency from verified saved receipts'
else
    bad "verified publication receipts did not produce actual latency: $status"
fi

# ---------------------------------------------------------------------------
# The rule itself: nothing in the release path may locate a process by pattern.
# This is the guard that stops a future wrapper from reintroducing
# `pgrep -f 'release-gate.sh <version>' | head -1`.
# ---------------------------------------------------------------------------
# Comments are allowed to name the banned practice — that is how the rule is
# explained. Only executable lines are a violation.
pattern_hits="$(grep -rnE '\b(pgrep|pkill)\b' \
    "$repo_root"/scripts/release-*.sh "$repo_root"/scripts/test-release-*.sh \
    2>/dev/null | grep -v 'test-release-train.sh' | grep -vE ':[0-9]+:[[:space:]]*#' || true)"
if [[ -z "$pattern_hits" ]]; then
    ok 'no release script locates a process by name pattern'
else
    bad "pattern-based process matching in the release path: $pattern_hits"
fi

for flavour in skills codex/skills grok/skills; do
    skill="$repo_root/cas-cli/src/builtins/$flavour/cas-cut-release/SKILL.md"
    if grep -q 'release-train.sh' "$skill" 2>/dev/null; then
        ok "cas-cut-release ($flavour) points at release-train.sh"
    else
        bad "cas-cut-release ($flavour) does not mention release-train.sh"
    fi
    # The rule has to be stated, not merely implied by the tooling: an operator
    # improvising a wrapper is exactly how the version-keyed path came back.
    if grep -qi 'recorded pid' "$skill" 2>/dev/null; then
        ok "cas-cut-release ($flavour) states the recorded-PID rule"
    else
        bad "cas-cut-release ($flavour) does not state the recorded-PID rule"
    fi
    if grep -qi 'never.*version-keyed\|version-keyed.*never\|per-run directory' "$skill" 2>/dev/null; then
        ok "cas-cut-release ($flavour) forbids a version-keyed artifacts path"
    else
        bad "cas-cut-release ($flavour) does not forbid a version-keyed artifacts path"
    fi
    for marker in 'Scoped Validation' 'ledger is the last prep step' 'scratch-base' \
        'detached process group' 'runtime_fixture_parent' 'reviewed snapshot update' \
        '9.99.x' 'cause class' 'workers never poll CI' 'competing release' \
        'merge-queue GraphQL query' 'CAS_RELEASE_ENV_FILE' 'annotated tag peels' \
        'four Slack POSTED' 'refresh_binary_version' 'stranded_branch_override' \
        'release.tag-complete.epoch' 'release-published.receipt'; do
        if grep -qF "$marker" "$skill" 2>/dev/null; then
            ok "cas-cut-release ($flavour) carries marker: $marker"
        else
            bad "cas-cut-release ($flavour) missing marker: $marker"
        fi
    done
done


# ===========================================================================
# `pipeline` — the port of the hand-written pipeline.sh (cas-da81).
#
# The centre of this suite is the mistake that dropped a merge-queue entry on
# 2026-09-04: a push-triggered CI run contributes rows with the required check
# NAMES but bucket "skipped", and treating those as satisfied enqueued the PR
# before the pull_request run existed, so the entry vanished silently.
# ===========================================================================

# A stub `gh` that answers from a scripted sequence. Each invocation appends
# its argv to calls.log and reads the current step from step.txt, so a test can
# make the same query answer differently on later polls.
new_gh_stub() {
    local path="$1" state_dir="$2"
    mkdir -p "$state_dir"
    cat >"$path" <<'STUB'
#!/usr/bin/env bash
state="$GH_STUB_STATE"
printf '%s\n' "$*" >> "$state/calls.log"
step="$(cat "$state/step.txt" 2>/dev/null || echo 1)"
printf '%s\n' "$((step + 1))" > "$state/step.txt"
case "$1 $2" in
"pr list")
    cat "$state/pr-list.json" 2>/dev/null || printf ''
    ;;
"pr create")
    printf 'https://github.com/o/r/pull/%s\n' "$(cat "$state/pr-number.txt" 2>/dev/null || echo 4242)"
    ;;
"pr comment")
    cat > /dev/null
    printf 'commented\n'
    ;;
"pr checks")
    if [[ -f "$state/checks-$step.json" ]]; then cat "$state/checks-$step.json";
    else cat "$state/checks-default.json" 2>/dev/null || printf '[]\n'; fi
    ;;
"pr view")
    if [[ -f "$state/prview-$step.json" ]]; then cat "$state/prview-$step.json";
    else cat "$state/prview-default.json" 2>/dev/null || printf '{}\n'; fi
    ;;
"api graphql")
    if printf '%s' "$*" | grep -q enqueuePullRequest; then
        cat "$state/enqueue.json" 2>/dev/null || printf '{"data":{"enqueuePullRequest":{"mergeQueueEntry":{"state":"QUEUED"}}}}\n'
    else
        if [[ -f "$state/entry-$step.txt" ]]; then cat "$state/entry-$step.txt";
        else cat "$state/entry-default.txt" 2>/dev/null || printf 'QUEUED\n'; fi
    fi
    ;;
"run list")
    if [[ -f "$state/runlist-$step.json" ]]; then cat "$state/runlist-$step.json";
    else cat "$state/runlist-default.json" 2>/dev/null || printf '[]\n'; fi
    ;;
*)
    printf ''
    ;;
esac
STUB
    chmod +x "$path"
}

# An epic worktree on a branch, with a real bare remote so the push is genuine.
new_pipeline_fixture() {
    local name="$1"
    local dir="$tmp/$name"
    local remote="$tmp/$name-remote.git"
    git init -q --bare "$remote"
    mkdir -p "$dir"
    ( cd "$dir"
      git init -q -b main .
      git config user.email test@test.invalid
      git config user.name 'Release Train Test'
      echo seed > seed.txt
      git add seed.txt
      git -c commit.gpgsign=false commit -q -m seed
      git checkout -q -b "epic/epic-$name"
      git remote add origin "$remote" ) >/dev/null
    printf '%s\n' "$dir"
}

pipeline_run_dir() { "$train" 9.99.9 "$1" --print-run-dir; }

seed_gate_receipt() {
    local run_dir="$1" worktree="$2" status="${3:-0}"
    mkdir -p "$run_dir"
    printf 'PASS version-literals\nPASS nextest\n' > "$run_dir/gate.log"
    printf '%s\n' "$status" > "$run_dir/gate.done"
    if [[ "$status" == 0 ]]; then
        git -C "$worktree" rev-parse HEAD >"$run_dir/gate.full.sha"
    fi
    printf 'release body\n' > "$run_dir/pr-body.md"
}

run_pipeline() {
    local worktree="$1" state="$2"
    GH_STUB_STATE="$state" \
    CAS_RELEASE_TRAIN_GH="$tmp/gh-stub.sh" \
    CAS_RELEASE_TRAIN_POLL_SECS=0 \
    CAS_RELEASE_TRAIN_CHECK_TRIES=4 \
    CAS_RELEASE_TRAIN_WATCH_TRIES=6 \
        "$train" 9.99.9 "$worktree" --pipeline 2>&1
}

new_gh_stub "$tmp/gh-stub.sh" "$tmp/gh-state-unused"

# --- refuses while the gate is not green -----------------------------------
wt_gate="$(new_pipeline_fixture gate-not-green)"
run_gate_dir="$(pipeline_run_dir "$wt_gate")"
seed_gate_receipt "$run_gate_dir" "$wt_gate" 1
state="$tmp/state-gate"; mkdir -p "$state"
out="$(run_pipeline "$wt_gate" "$state" || true)"
if [[ "$out" == *"GATE_NOT_GREEN"* ]]; then
    ok 'pipeline refuses to run while the gate is not green'
else
    bad "pipeline ran without a green gate: $out"
fi

# A successful diagnostic row without a full-gate receipt cannot authorize a
# push. The exact remote stays empty, proving refusal happened before mutation.
wt_partial="$(new_pipeline_fixture partial-only-receipt)"
run_partial_dir="$(pipeline_run_dir "$wt_partial")"
mkdir -p "$run_partial_dir/diagnostics/fixture"
printf 'PASS nextest\n' >"$run_partial_dir/diagnostics/fixture/gate.log"
printf '0\n' >"$run_partial_dir/diagnostics/fixture/gate.done"
printf 'release body\n' >"$run_partial_dir/pr-body.md"
state="$tmp/state-partial"; mkdir -p "$state"
out="$(run_pipeline "$wt_partial" "$state" || true)"
if [[ "$out" == *"GATE_NOT_GREEN"* ]] \
    && [[ -z "$(git -C "$wt_partial" ls-remote --heads origin)" ]]; then
    ok 'a green --only diagnostic receipt cannot authorize pipeline push'
else
    bad "partial diagnostic authorized or reached a push: $out"
fi

# A full-gate receipt is bound to one exact tree. Any later commit makes it
# stale and must refuse before pushing the changed tree.
wt_stale_gate="$(new_pipeline_fixture stale-full-gate-receipt)"
run_stale_gate_dir="$(pipeline_run_dir "$wt_stale_gate")"
seed_gate_receipt "$run_stale_gate_dir" "$wt_stale_gate"
printf 'changed after gate\n' >"$wt_stale_gate/after-gate.txt"
git -C "$wt_stale_gate" add after-gate.txt
git -C "$wt_stale_gate" -c commit.gpgsign=false commit -q -m 'change after full gate'
state="$tmp/state-stale-gate"; mkdir -p "$state"
out="$(run_pipeline "$wt_stale_gate" "$state" || true)"
if [[ "$out" == *"STALE_FULL_GATE"* ]] \
    && [[ -z "$(git -C "$wt_stale_gate" ls-remote --heads origin)" ]]; then
    ok 'pipeline rejects a changed tree with a stale full-gate receipt before push'
else
    bad "stale full-gate receipt authorized or reached a push: $out"
fi

# --- the incident: SKIPPED rows must not satisfy the required checks -------
wt_skip="$(new_pipeline_fixture skipped-rows)"
run_skip_dir="$(pipeline_run_dir "$wt_skip")"
seed_gate_receipt "$run_skip_dir" "$wt_skip"
state="$tmp/state-skip"; mkdir -p "$state"
printf '' > "$state/pr-list.json"
printf '4242\n' > "$state/pr-number.txt"
# Every checks poll returns the push-triggered run's SKIPPED rows only.
cat > "$state/checks-default.json" <<'JSON'
[{"name":"Fast Validation","bucket":"skipped"},{"name":"macOS Check","bucket":"skipped"}]
JSON
printf '{"state":"OPEN","mergeCommit":null,"id":"PR_id"}\n' > "$state/prview-default.json"
out="$(run_pipeline "$wt_skip" "$state" || true)"
if [[ "$out" == *"CHECKS_NEVER_PASSED"* || "$(cat "$run_skip_dir/pipeline.done" 2>/dev/null)" == "CHECKS_FAILED" ]]; then
    ok 'SKIPPED rows from a push-triggered run do not satisfy the required checks'
else
    bad "skipped rows were treated as passing: $out"
fi
if ! grep -q 'enqueuePullRequest' "$state/calls.log" 2>/dev/null; then
    ok 'no enqueue is attempted before the required checks pass'
else
    bad 'the pipeline enqueued before the required checks passed'
fi

# --- happy path: create PR, comment, wait, enqueue, watch to MERGED --------
wt_ok="$(new_pipeline_fixture happy-path)"
run_ok_dir="$(pipeline_run_dir "$wt_ok")"
seed_gate_receipt "$run_ok_dir" "$wt_ok"
state="$tmp/state-ok"; mkdir -p "$state"
printf '' > "$state/pr-list.json"
printf '4242\n' > "$state/pr-number.txt"
cat > "$state/checks-default.json" <<'JSON'
[{"name":"Fast Validation","bucket":"pass"},{"name":"macOS Check","bucket":"pass"}]
JSON
printf '{"state":"MERGED","mergeCommit":{"oid":"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"},"id":"PR_id"}\n' > "$state/prview-default.json"
out="$(run_pipeline "$wt_ok" "$state" || true)"
if [[ "$(cat "$run_ok_dir/pipeline.done" 2>/dev/null)" == "MERGED" ]]; then
    ok 'a merged PR ends the pipeline with MERGED'
else
    bad "pipeline.done is $(cat "$run_ok_dir/pipeline.done" 2>/dev/null || echo absent): $out"
fi
if [[ "$(cat "$run_ok_dir/pr-number.txt" 2>/dev/null)" == "4242" ]]; then
    ok 'the PR number is recorded in the run directory'
else
    bad "pr-number.txt is $(cat "$run_ok_dir/pr-number.txt" 2>/dev/null || echo absent)"
fi
if [[ -s "$run_ok_dir/landed-main.sha" ]]; then
    ok 'the landed main sha is recorded for the publish step'
else
    bad 'landed-main.sha was not recorded'
fi
if grep -q 'pr comment' "$state/calls.log"; then
    ok 'the gate receipt is commented on the PR'
else
    bad 'no gate receipt comment was posted'
fi

# --- an existing PR is reused, never duplicated ----------------------------
wt_reuse="$(new_pipeline_fixture reuse-pr)"
run_reuse_dir="$(pipeline_run_dir "$wt_reuse")"
seed_gate_receipt "$run_reuse_dir" "$wt_reuse"
state="$tmp/state-reuse"; mkdir -p "$state"
printf '[{"number":777}]\n' > "$state/pr-list.json"
cat > "$state/checks-default.json" <<'JSON'
[{"name":"Fast Validation","bucket":"pass"},{"name":"macOS Check","bucket":"pass"}]
JSON
printf '{"state":"MERGED","mergeCommit":{"oid":"cafebabecafebabecafebabecafebabecafebabe"},"id":"PR_id"}\n' > "$state/prview-default.json"
run_pipeline "$wt_reuse" "$state" >/dev/null 2>&1 || true
if [[ "$(cat "$run_reuse_dir/pr-number.txt" 2>/dev/null)" == "777" ]] && ! grep -q '^pr create' "$state/calls.log"; then
    ok 'an existing PR for the head branch is reused, not recreated'
else
    bad "existing PR was not reused: $(cat "$run_reuse_dir/pr-number.txt" 2>/dev/null)"
fi

# --- a dropped merge-queue entry is re-enqueued, then given up on ----------
wt_drop="$(new_pipeline_fixture dropped-entry)"
run_drop_dir="$(pipeline_run_dir "$wt_drop")"
seed_gate_receipt "$run_drop_dir" "$wt_drop"
state="$tmp/state-drop"; mkdir -p "$state"
printf '' > "$state/pr-list.json"
printf '4242\n' > "$state/pr-number.txt"
cat > "$state/checks-default.json" <<'JSON'
[{"name":"Fast Validation","bucket":"pass"},{"name":"macOS Check","bucket":"pass"}]
JSON
printf '{"state":"OPEN","mergeCommit":null,"id":"PR_id"}\n' > "$state/prview-default.json"
printf 'no-entry\n' > "$state/entry-default.txt"
printf '[]\n' > "$state/runlist-default.json"
out="$(run_pipeline "$wt_drop" "$state" || true)"
if [[ "$(cat "$run_drop_dir/pipeline.done" 2>/dev/null)" == "DROPPED_TOO_OFTEN" ]]; then
    ok 'an entry that keeps vanishing ends as DROPPED_TOO_OFTEN'
else
    bad "pipeline.done is $(cat "$run_drop_dir/pipeline.done" 2>/dev/null || echo absent): $out"
fi
requeues="$(grep -c 'enqueuePullRequest' "$state/calls.log" || true)"
if [[ "$requeues" -ge 2 && "$requeues" -le 4 ]]; then
    ok "a dropped entry is re-enqueued a bounded number of times ($requeues)"
else
    bad "unexpected enqueue count: $requeues"
fi

# --- a failed merge_group run is terminal ----------------------------------
wt_qfail="$(new_pipeline_fixture queue-failed)"
run_qfail_dir="$(pipeline_run_dir "$wt_qfail")"
seed_gate_receipt "$run_qfail_dir" "$wt_qfail"
state="$tmp/state-qfail"; mkdir -p "$state"
printf '' > "$state/pr-list.json"
printf '4242\n' > "$state/pr-number.txt"
cat > "$state/checks-default.json" <<'JSON'
[{"name":"Fast Validation","bucket":"pass"},{"name":"macOS Check","bucket":"pass"}]
JSON
printf '{"state":"OPEN","mergeCommit":null,"id":"PR_id"}\n' > "$state/prview-default.json"
printf 'QUEUED\n' > "$state/entry-default.txt"
cat > "$state/runlist-default.json" <<'JSON'
[{"databaseId":99,"status":"completed","conclusion":"failure","createdAt":"2099-01-01T00:00:00Z"}]
JSON
out="$(run_pipeline "$wt_qfail" "$state" || true)"
if [[ "$(cat "$run_qfail_dir/pipeline.done" 2>/dev/null)" == "QUEUE_RUN_FAILED" ]]; then
    ok 'a failed merge_group run ends as QUEUE_RUN_FAILED'
else
    bad "pipeline.done is $(cat "$run_qfail_dir/pipeline.done" 2>/dev/null || echo absent): $out"
fi


# --- merge_group runs are judged from the ENQUEUE time, not pipeline start ---
# A failed queue run from an earlier attempt must not condemn this one: the
# SINCE cursor is captured when the enqueue happens (and reset on every
# re-enqueue), so anything older is another attempt's history.
wt_stale="$(new_pipeline_fixture stale-queue-run)"
run_stale_dir="$(pipeline_run_dir "$wt_stale")"
seed_gate_receipt "$run_stale_dir" "$wt_stale"
state="$tmp/state-stale"; mkdir -p "$state"
printf '' > "$state/pr-list.json"
printf '4242\n' > "$state/pr-number.txt"
cat > "$state/checks-default.json" <<'JSON'
[{"name":"Fast Validation","bucket":"pass"},{"name":"macOS Check","bucket":"pass"}]
JSON
printf '{"state":"MERGED","mergeCommit":{"oid":"f00dcafef00dcafef00dcafef00dcafef00dcafe"},"id":"PR_id"}\n' > "$state/prview-default.json"
cat > "$state/runlist-default.json" <<'JSON'
[{"databaseId":7,"status":"completed","conclusion":"failure","createdAt":"2020-01-01T00:00:00Z"}]
JSON
out="$(run_pipeline "$wt_stale" "$state" || true)"
if [[ "$(cat "$run_stale_dir/pipeline.done" 2>/dev/null)" == "MERGED" ]]; then
    ok 'a merge_group failure older than the enqueue is ignored'
else
    bad "stale queue run condemned this attempt: $(cat "$run_stale_dir/pipeline.done" 2>/dev/null): $out"
fi

# --- the run directory carries a tailable, UTC-timestamped pipeline log -----
if [[ -s "$run_stale_dir/pipeline.log" ]] \
   && grep -qE '^[0-9]{2}:[0-9]{2}:[0-9]{2}Z ' "$run_stale_dir/pipeline.log" \
   && grep -q 'pipeline terminal state' "$run_stale_dir/pipeline.log"; then
    ok 'pipeline.log records UTC-timestamped lines through the terminal state'
else
    bad "pipeline.log is missing, unstamped, or truncated: $(head -3 "$run_stale_dir/pipeline.log" 2>/dev/null || echo absent)"
fi


# ===========================================================================
# `publish` — the port of publish-wrapper.sh (cas-c1cd).
#
# Everything that can refuse must refuse BEFORE a tag worktree exists or a
# publish process starts: publishing the wrong tree is not recoverable by
# retrying, and the receipts must say which process actually ran.
# ===========================================================================

# A fixture whose origin/main really carries the landed commit, plus a
# cas-cli/Cargo.toml the version check can read.
new_publish_fixture() {
    local name="$1" version="$2"
    local dir="$tmp/$name"
    local remote="$tmp/$name-remote.git"
    git init -q --bare "$remote"
    mkdir -p "$dir"
    ( cd "$dir"
      git init -q -b main .
      git config user.email test@test.invalid
      git config user.name 'Release Train Test'
      mkdir -p cas-cli scripts
      printf 'version = "%s"\n' "$version" > cas-cli/Cargo.toml
      printf 'seed\n' > seed.txt
      git add -A
      git -c commit.gpgsign=false commit -q -m "release $version"
      git remote add origin "$remote"
      git push -q origin main ) >/dev/null
    printf '%s\n' "$dir"
}

new_publish_stub() {
    local path="$1" exit_code="$2"
    cat >"$path" <<EOF
#!/usr/bin/env bash
printf 'stub publisher args=%s cwd=%s\n' "\$*" "\$PWD"
exit $exit_code
EOF
    chmod +x "$path"
}

run_publish() {
    local worktree="$1" version="$2" sha="$3" publish_cmd="$4" env_file="$5"
    CAS_RELEASE_TRAIN_PUBLISH_CMD="$publish_cmd" \
    CAS_RELEASE_ENV_FILE="$env_file" \
        "$train" "$version" "$worktree" --publish "$sha" 2>&1
}

pub_env="$tmp/release.env"
printf 'CAS_TEST_TOKEN=super-secret-value\nCAS_TEST_OTHER=another-secret\n' > "$pub_env"

# --- happy path: receipts land in the run dir and name the real status -----
wt_pub="$(new_publish_fixture publish-ok 9.99.9)"
run_pub_dir="$(pipeline_run_dir "$wt_pub")"
mkdir -p "$run_pub_dir"
landed="$(git -C "$wt_pub" rev-parse HEAD)"
new_publish_stub "$tmp/publisher-ok.sh" 0
out="$(run_publish "$wt_pub" 9.99.9 "$landed" "$tmp/publisher-ok.sh" "$pub_env" || true)"

if [[ "$(cat "$run_pub_dir/release.done" 2>/dev/null)" == "0" ]]; then
    ok 'a successful publish records release.done=0 in the run directory'
else
    bad "release.done is $(cat "$run_pub_dir/release.done" 2>/dev/null || echo absent): $out"
fi
if [[ -s "$run_pub_dir/release.tag-complete.epoch" ]] \
    && [[ ! -e "$run_pub_dir/release.published.epoch" ]] \
    && [[ ! -e "$run_pub_dir/release-published.receipt" ]]; then
    ok 'tag publisher success records only tag completion, never publication'
else
    bad 'tag publisher success manufactured a publication receipt'
fi
if [[ -s "$run_pub_dir/release.pid" ]] && [[ -s "$run_pub_dir/release.log" ]]; then
    ok 'the publisher PID and log are recorded in the run directory'
else
    bad 'release.pid or release.log missing from the run directory'
fi
if grep -q 'stub publisher' "$run_pub_dir/release.log" 2>/dev/null; then
    ok 'the publisher output is captured'
else
    bad "release.log does not hold the publisher output: $(cat "$run_pub_dir/release.log" 2>/dev/null || echo absent)"
fi
if [[ "$out" == *"CAS_TEST_TOKEN"* && "$out" != *"super-secret-value"* ]]; then
    ok 'the credential proof prints variable names but never values'
else
    bad 'the credential proof leaked a value or named nothing'
fi

# --- a failing publisher is recorded, not swallowed ------------------------
wt_fail="$(new_publish_fixture publish-fails 9.99.9)"
run_fail_dir="$(pipeline_run_dir "$wt_fail")"
mkdir -p "$run_fail_dir"
landed_fail="$(git -C "$wt_fail" rev-parse HEAD)"
new_publish_stub "$tmp/publisher-bad.sh" 7
run_publish "$wt_fail" 9.99.9 "$landed_fail" "$tmp/publisher-bad.sh" "$pub_env" >/dev/null 2>&1 || true
if [[ "$(cat "$run_fail_dir/release.done" 2>/dev/null)" == "7" ]]; then
    ok 'a failing publisher exit status is recorded verbatim'
else
    bad "release.done is $(cat "$run_fail_dir/release.done" 2>/dev/null || echo absent), expected 7"
fi

# --- refusals happen before any worktree or publisher exists ---------------
wt_sha="$(new_publish_fixture publish-sha-mismatch 9.99.9)"
run_sha_dir="$(pipeline_run_dir "$wt_sha")"
mkdir -p "$run_sha_dir"
out="$(run_publish "$wt_sha" 9.99.9 0000000000000000000000000000000000000000 "$tmp/publisher-ok.sh" "$pub_env" || true)"
if [[ "$out" == *"origin/main"* ]] && [[ ! -e "$run_sha_dir/release.done" ]] \
   && [[ ! -d "$wt_sha/.cas/release-v9.99.9" ]]; then
    ok 'a landed sha that is not origin/main refuses before creating a worktree'
else
    bad "sha mismatch did not refuse cleanly: $out"
fi

wt_ver="$(new_publish_fixture publish-version-mismatch 9.99.3)"
run_ver_dir="$(pipeline_run_dir "$wt_ver")"
mkdir -p "$run_ver_dir"
landed_ver="$(git -C "$wt_ver" rev-parse HEAD)"
out="$(run_publish "$wt_ver" 9.99.9 "$landed_ver" "$tmp/publisher-ok.sh" "$pub_env" || true)"
if [[ "$out" == *"9.99.3"* && "$out" == *"9.99.9"* ]] && [[ ! -e "$run_ver_dir/release.done" ]]; then
    ok 'a version mismatch refuses and names both the expected and actual version'
else
    bad "version mismatch did not refuse with both versions: $out"
fi

# --- the sha defaults to what the pipeline already recorded ----------------
wt_default="$(new_publish_fixture publish-default-sha 9.99.9)"
run_default_dir="$(pipeline_run_dir "$wt_default")"
mkdir -p "$run_default_dir"
git -C "$wt_default" rev-parse HEAD > "$run_default_dir/landed-main.sha"
CAS_RELEASE_TRAIN_PUBLISH_CMD="$tmp/publisher-ok.sh" CAS_RELEASE_ENV_FILE="$pub_env" \
    "$train" 9.99.9 "$wt_default" --publish >/dev/null 2>&1 || true
if [[ "$(cat "$run_default_dir/release.done" 2>/dev/null)" == "0" ]]; then
    ok 'publish falls back to the landed-main.sha the pipeline recorded'
else
    bad "publish did not use the recorded landed sha: $(cat "$run_default_dir/release.done" 2>/dev/null || echo absent)"
fi

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0

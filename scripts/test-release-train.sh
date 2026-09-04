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
      git add seed.txt
      git -c commit.gpgsign=false commit -q -m seed ) >/dev/null
    printf '%s\n' "$dir"
}

# A stub standing in for release-gate.sh: it records that it ran and how, then
# exits with the status the fixture asked for.
new_gate_stub() {
    local path="$1" exit_code="$2" sleep_for="${3:-0}"
    cat >"$path" <<EOF
#!/usr/bin/env bash
printf 'stub gate version=%s cwd=%s\n' "\$1" "\$PWD"
sleep $sleep_for
exit $exit_code
EOF
    chmod +x "$path"
}

# ---------------------------------------------------------------------------
# The run directory is keyed by the worktree, not by the version alone.
# ---------------------------------------------------------------------------
wt_a="$(new_worktree epic-a-merge)"
wt_b="$(new_worktree epic-b-merge)"

dir_a="$("$train" 3.15.2 "$wt_a" --print-run-dir)"
dir_b="$("$train" 3.15.2 "$wt_b" --print-run-dir)"

if [[ "$dir_a" != "$dir_b" ]]; then
    ok 'two worktrees at the same version resolve to different run directories'
else
    bad "both worktrees resolved to $dir_a"
fi
if [[ "$dir_a" == *"3.15.2"* && "$dir_a" == *"epic-a-merge"* ]]; then
    ok 'the run directory names both the version and the worktree'
else
    bad "run directory does not identify the run: $dir_a"
fi

# ---------------------------------------------------------------------------
# A completed gate leaves an attributable receipt.
# ---------------------------------------------------------------------------
gate_ok="$tmp/gate-ok.sh"
new_gate_stub "$gate_ok" 0
CAS_RELEASE_TRAIN_GATE_CMD="$gate_ok" "$train" 3.15.2 "$wt_a" --gate >/dev/null 2>&1 || true

if [[ "$(cat "$dir_a/gate.done" 2>/dev/null)" == "0" ]]; then
    ok 'a successful gate records its exit status in gate.done'
else
    bad "gate.done missing or non-zero: $(cat "$dir_a/gate.done" 2>/dev/null || echo absent)"
fi
if grep -q "$wt_a" "$dir_a/run.env" 2>/dev/null; then
    ok 'run.env attributes the run to its worktree'
else
    bad "run.env does not name the worktree: $(cat "$dir_a/run.env" 2>/dev/null || echo absent)"
fi
if grep -q "stub gate version=3.15.2" "$dir_a/gate.log" 2>/dev/null; then
    ok 'the gate log lands in the run directory'
else
    bad "gate.log missing or empty: $(cat "$dir_a/gate.log" 2>/dev/null || echo absent)"
fi

# ---------------------------------------------------------------------------
# A second start refuses while the first run's recorded pid is alive, and says
# whose run it is and what to do about it.
# ---------------------------------------------------------------------------
gate_slow="$tmp/gate-slow.sh"
new_gate_stub "$gate_slow" 0 30
CAS_RELEASE_TRAIN_GATE_CMD="$gate_slow" "$train" 3.15.2 "$wt_b" --gate >/dev/null 2>&1 &
runner_b=$!
for _ in $(seq 1 50); do
    [[ -f "$dir_b/gate.pid" ]] && break
    sleep 0.1
done
held_pid="$(cat "$dir_b/gate.pid" 2>/dev/null || true)"

refusal="$(CAS_RELEASE_TRAIN_GATE_CMD="$gate_ok" "$train" 3.15.2 "$wt_b" --gate 2>&1 || true)"
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
CAS_RELEASE_TRAIN_GATE_CMD="$gate_slow" "$train" 3.15.2 "$wt_a" --gate >/dev/null 2>&1 &
runner_a=$!
sibling_pid=""
for _ in $(seq 1 50); do
    candidate="$(cat "$dir_a/gate.pid" 2>/dev/null || true)"
    if [[ -n "$candidate" ]] && kill -0 "$candidate" 2>/dev/null; then
        sibling_pid="$candidate"
        break
    fi
    sleep 0.1
done

"$train" 3.15.2 "$wt_b" --stop >/dev/null 2>&1 || true
sleep 0.5

if [[ -n "$held_pid" ]] && ! kill -0 "$held_pid" 2>/dev/null; then
    ok '--stop terminates the run it recorded'
else
    bad "--stop did not terminate its own gate (pid $held_pid)"
fi
if [[ -n "$sibling_pid" ]] && kill -0 "$sibling_pid" 2>/dev/null; then
    ok 'a concurrent run for another worktree survives its sibling being stopped'
else
    bad "the sibling run (pid $sibling_pid) died with its sibling"
fi

"$train" 3.15.2 "$wt_a" --stop >/dev/null 2>&1 || true
wait "$runner_a" 2>/dev/null || true
wait "$runner_b" 2>/dev/null || true

status="$("$train" 3.15.2 "$wt_a" --status 2>&1 || true)"
if [[ "$status" == *"$dir_a"* ]]; then
    ok '--status reports the run directory it is talking about'
else
    bad "--status did not identify the run: $status"
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

pipeline_run_dir() { "$train" 9.9.9 "$1" --print-run-dir; }

seed_gate_receipt() {
    local run_dir="$1" status="${2:-0}"
    mkdir -p "$run_dir"
    printf 'PASS version-literals\nPASS nextest\n' > "$run_dir/gate.log"
    printf '%s\n' "$status" > "$run_dir/gate.done"
    printf 'release body\n' > "$run_dir/pr-body.md"
}

run_pipeline() {
    local worktree="$1" state="$2"
    GH_STUB_STATE="$state" \
    CAS_RELEASE_TRAIN_GH="$tmp/gh-stub.sh" \
    CAS_RELEASE_TRAIN_POLL_SECS=0 \
    CAS_RELEASE_TRAIN_CHECK_TRIES=4 \
    CAS_RELEASE_TRAIN_WATCH_TRIES=6 \
        "$train" 9.9.9 "$worktree" --pipeline 2>&1
}

new_gh_stub "$tmp/gh-stub.sh" "$tmp/gh-state-unused"

# --- refuses while the gate is not green -----------------------------------
wt_gate="$(new_pipeline_fixture gate-not-green)"
run_gate_dir="$(pipeline_run_dir "$wt_gate")"
seed_gate_receipt "$run_gate_dir" 1
state="$tmp/state-gate"; mkdir -p "$state"
out="$(run_pipeline "$wt_gate" "$state" || true)"
if [[ "$out" == *"GATE_NOT_GREEN"* ]]; then
    ok 'pipeline refuses to run while the gate is not green'
else
    bad "pipeline ran without a green gate: $out"
fi

# --- the incident: SKIPPED rows must not satisfy the required checks -------
wt_skip="$(new_pipeline_fixture skipped-rows)"
run_skip_dir="$(pipeline_run_dir "$wt_skip")"
seed_gate_receipt "$run_skip_dir"
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
seed_gate_receipt "$run_ok_dir"
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
seed_gate_receipt "$run_reuse_dir"
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
seed_gate_receipt "$run_drop_dir"
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
seed_gate_receipt "$run_qfail_dir"
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
seed_gate_receipt "$run_stale_dir"
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

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0

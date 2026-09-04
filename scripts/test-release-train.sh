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
dir_env="$("$train" 3.15.2 "$wt_env" --print-run-dir)"
CAS_RELEASE_TRAIN_GATE_CMD="$gate_env" CAS_INIT_TIMEOUT_SECS=900 \
    "$train" 3.15.2 "$wt_env" --gate >/dev/null 2>&1 || true

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

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0

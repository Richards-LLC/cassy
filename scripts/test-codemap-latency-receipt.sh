#!/usr/bin/env bash
# Deterministic self-test for scripts/codemap-latency-receipt.sh.
set -uo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
receipt="$script_dir/codemap-latency-receipt.sh"
tmp="$(mktemp -d)"
named_worktree=""
named_branch=""
detached_worktree=""
cleanup() {
    if [[ -n "$detached_worktree" ]]; then
        git worktree remove --force "$detached_worktree" >/dev/null 2>&1 || true
    fi
    if [[ -n "$named_worktree" ]]; then
        git worktree remove --force "$named_worktree" >/dev/null 2>&1 || true
    fi
    if [[ -n "$named_branch" ]]; then
        git branch -D "$named_branch" >/dev/null 2>&1 || true
    fi
    rm -rf "$tmp"
}
trap cleanup EXIT

pass=0
fail=0

ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }

expect_field() {
    output="$1"
    field="$2"
    expected="$3"
    label="$4"
    actual="$(grep -m1 "^$field=" <<<"$output" | cut -d= -f2-)"
    if [[ "$actual" == "$expected" ]]; then
        ok "$label"
    else
        bad "$label (expected $field=$expected; got $actual)"
    fi
}

cat >"$tmp/fake-clock" <<'EOF'
#!/usr/bin/env bash
state="${CLOCK_STATE:?}"
step="${CLOCK_STEP:-1}"
count=0
if [[ -f "$state" ]]; then count="$(<"$state")"; fi
printf '%s\n' $((count + step)) >"$state"
printf '%s\n' $((count + step))
EOF
chmod +x "$tmp/fake-clock"

cat >"$tmp/fake-cas" <<'EOF'
#!/usr/bin/env bash
case "$1 $2" in
  "codemap status")
    case "${FAKE_CODEMAP_STATUS:-up-to-date}" in
      up-to-date)
        printf '%s\n' 'CODEMAP.md: /tmp/project/.claude/CODEMAP.md' '  Status: up to date'
        ;;
      stale)
        printf '%s\n' 'CODEMAP.md: /tmp/project/.claude/CODEMAP.md' '  Status: stale'
        ;;
      missing)
        printf '%s\n' 'CODEMAP.md: not found'
        ;;
      *)
        echo "unexpected fake codemap status: ${FAKE_CODEMAP_STATUS}" >&2
        exit 2
        ;;
    esac
    ;;
  "knowledge build")
    printf '%s\n' 'knowledge build completed'
    ;;
  *)
    echo "unexpected fake cas invocation: $*" >&2
    exit 2
    ;;
esac
EOF
chmod +x "$tmp/fake-cas"

cat >"$tmp/fake-gh" <<'EOF'
#!/usr/bin/env bash
if [[ "$1 $2" == 'run view' ]]; then
    mac_end='2026-08-30T12:00:09Z'
    if [[ "${FAKE_REQUIRED_SECONDS:-5}" == 60 ]]; then
        mac_end='2026-08-30T12:01:04Z'
    fi
    cat <<JSON
{"createdAt":"2026-08-30T12:00:00Z","url":"https://github.com/example/repo/actions/runs/123","jobs":[
  {"name":"Fast Validation","startedAt":"2026-08-30T12:00:03Z","completedAt":"2026-08-30T12:00:05Z","conclusion":"success"},
  {"name":"macOS Check","startedAt":"2026-08-30T12:00:04Z","completedAt":"$mac_end","conclusion":"success"},
  {"name":"Clippy (advisory)","startedAt":"2026-08-30T12:00:03Z","completedAt":"2026-08-30T12:01:00Z","conclusion":"skipped"}
]}
JSON
else
    echo "unexpected fake gh invocation: $*" >&2
    exit 2
fi
EOF
chmod +x "$tmp/fake-gh"

named_worktree="$tmp/named-worktree"
named_branch="codemap-latency-self-test-$$"
git worktree add --quiet -b "$named_branch" "$named_worktree" HEAD

export CAS_BIN="$tmp/fake-cas"
export GH_BIN="$tmp/fake-gh"
export CLOCK_STATE="$tmp/clock-state"
export CODEMAP_LATENCY_CLOCK="$tmp/fake-clock"

artifact="$tmp/receipt.env"
out="$(CLOCK_STEP=1 "$receipt" --repo-root "$named_worktree" --artifact "$artifact" --github-run-id 123 --github-repo example/repo)"
expect_field "$out" CODEMAP_RENDER_STATUS identical 'a no-op render is recognized as identical'
expect_field "$out" NO_CONTENT_CHANGE_PRESERVED true 'identical render preserves the no-content rule'
expect_field "$out" CODEMAP_FRESHNESS_STATUS up-to-date 'up-to-date freshness is recorded'
expect_field "$out" READINESS_MODE local-named-branch 'named-branch local readiness is labeled'
expect_field "$out" KNOWLEDGE_BUILD_BUDGET_SECONDS 90 'knowledge build defaults to a 90-second bound'
expect_field "$out" KNOWLEDGE_BUILD_WITHIN_BUDGET true 'knowledge build is within its bound'
expect_field "$out" AGENT_CONTROLLED_TOTAL_SECONDS 4 'agent-controlled phases exclude docs and queue time'
expect_field "$out" AGENT_CONTROLLED_WITHIN_BUDGET true 'agent-controlled total is within five minutes'
expect_field "$out" DOCS_ONLY_REQUIRED_COMPUTE_SECONDS 5 'required docs path uses the slower required context'
expect_field "$out" DOCS_ONLY_REQUIRED_COMPUTE_SOURCE github-required-jobs 'required docs timing identifies its source'
expect_field "$out" DOCS_ONLY_REQUIRED_COMPUTE_WITHIN_BUDGET true 'required docs path is within one minute'
expect_field "$out" GITHUB_QUEUE_SECONDS 3 'GitHub queue delay starts at workflow creation and ends at first runner'
expect_field "$out" GITHUB_QUEUE_STATUS measured 'GitHub queue timing is measured separately'
if cmp -s "$artifact" <(printf '%s\n' "$out" | sed '/^ARTIFACT_PATH=/d'); then
    ok 'artifact is byte-for-byte the emitted receipt'
else
    bad 'artifact differs from emitted receipt'
fi

# A changed candidate must fail before CODEMAP.md can be touched.
printf '%s\n' 'changed render' >"$tmp/changed-CODEMAP.md"
before_hash="$(git -C "$named_worktree" hash-object .claude/CODEMAP.md)"
set +e
CLOCK_STATE="$tmp/changed-clock-state" CLOCK_STEP=1 "$receipt" --repo-root "$named_worktree" --rendered-path "$tmp/changed-CODEMAP.md" >"$tmp/changed.out" 2>&1
changed_status=$?
set -e
after_hash="$(git -C "$named_worktree" hash-object .claude/CODEMAP.md)"
if [[ "$changed_status" -ne 0 ]]; then ok 'changed render exits non-zero'; else bad 'changed render exited zero'; fi
if [[ "$before_hash" == "$after_hash" ]]; then ok 'changed render never modifies fixture CODEMAP.md'; else bad 'changed render modified fixture CODEMAP.md'; fi

# Four local agent phases at 76 seconds each exceed 300 seconds while the
# bounded knowledge phase itself remains under 90 seconds.
set +e
CLOCK_STATE="$tmp/slow-clock-state" CLOCK_STEP=76 "$receipt" --repo-root "$named_worktree" --github-run-id 123 --github-repo example/repo >"$tmp/slow.out" 2>&1
slow_status=$?
set -e
slow_output="$(<"$tmp/slow.out")"
if [[ "$slow_status" -ne 0 ]]; then ok 'over-budget agent-controlled work exits non-zero'; else bad 'over-budget agent-controlled work exited zero'; fi
expect_field "$slow_output" AGENT_CONTROLLED_TOTAL_SECONDS 304 'agent total is the sum of the four local phases'
expect_field "$slow_output" AGENT_CONTROLLED_WITHIN_BUDGET false 'agent budget failure is explicit'
expect_field "$slow_output" KNOWLEDGE_BUILD_WITHIN_BUDGET true 'knowledge bound remains independent of agent total'

set +e
CODEMAP_AGENT_BUDGET_SECONDS=0 "$receipt" --repo-root "$named_worktree" >/dev/null 2>&1
invalid_status=$?
set -e
if [[ "$invalid_status" -eq 2 ]]; then ok 'invalid budget is rejected'; else bad "invalid budget exited $invalid_status"; fi

for budget_case in \
    'CODEMAP_AGENT_BUDGET_SECONDS 301 agent budget above canonical maximum' \
    'CODEMAP_KNOWLEDGE_BUDGET_SECONDS 91 knowledge budget above canonical maximum' \
    'CODEMAP_DOCS_ONLY_BUDGET_SECONDS 61 docs-only budget above canonical maximum'; do
    read -r budget_name budget_value budget_label <<<"$budget_case"
    set +e
    env "$budget_name=$budget_value" "$receipt" --repo-root "$named_worktree" >/dev/null 2>&1
    budget_status=$?
    set -e
    if [[ "$budget_status" -eq 2 ]]; then ok "$budget_label is rejected"; else bad "$budget_label exited $budget_status"; fi
done

# Canonical lower bounds remain usable when all measured phases are zero.
set +e
lower_output="$(
    CODEMAP_AGENT_BUDGET_SECONDS=1 \
    CODEMAP_KNOWLEDGE_BUDGET_SECONDS=1 \
    CODEMAP_DOCS_ONLY_BUDGET_SECONDS=1 \
    CLOCK_STATE="$tmp/lower-clock-state" CLOCK_STEP=0 \
    "$receipt" --repo-root "$named_worktree"
)"
lower_status=$?
set -e
if [[ "$lower_status" -eq 0 ]]; then ok 'positive lower-bound overrides are accepted'; else bad "positive lower-bound overrides exited $lower_status"; fi
expect_field "$lower_output" AGENT_CONTROLLED_BUDGET_SECONDS 1 'agent lower-bound override is retained'
expect_field "$lower_output" KNOWLEDGE_BUILD_BUDGET_SECONDS 1 'knowledge lower-bound override is retained'
expect_field "$lower_output" DOCS_ONLY_REQUIRED_COMPUTE_BUDGET_SECONDS 1 'docs-only lower-bound override is retained'

for freshness_case in stale missing; do
    set +e
    freshness_output="$(
        FAKE_CODEMAP_STATUS="$freshness_case" \
        CLOCK_STATE="$tmp/$freshness_case-clock-state" CLOCK_STEP=0 \
        "$receipt" --repo-root "$named_worktree"
    )"
    freshness_status=$?
    set -e
    if [[ "$freshness_status" -ne 0 ]]; then ok "$freshness_case freshness exits non-zero"; else bad "$freshness_case freshness exited zero"; fi
    expect_field "$freshness_output" CODEMAP_FRESHNESS_STATUS "$freshness_case" "$freshness_case freshness is recorded"
done

# The required docs compute contract is strict: exactly 60 seconds fails.
set +e
exact_output="$(
    FAKE_REQUIRED_SECONDS=60 \
    CLOCK_STATE="$tmp/exact-clock-state" CLOCK_STEP=0 \
    "$receipt" --repo-root "$named_worktree" --github-run-id 123 --github-repo example/repo
)"
exact_status=$?
set -e
if [[ "$exact_status" -ne 0 ]]; then ok 'exactly 60 seconds required compute exits non-zero'; else bad 'exactly 60 seconds required compute exited zero'; fi
expect_field "$exact_output" DOCS_ONLY_REQUIRED_COMPUTE_SECONDS 60 'exact-bound required compute is measured'
expect_field "$exact_output" DOCS_ONLY_REQUIRED_COMPUTE_WITHIN_BUDGET false 'exact-bound required compute fails strict docs contract'

# A detached checkout remains invalid for an ordinary local rehearsal, even
# when the test process itself has inherited the complete CI identity.
detached_worktree="$tmp/detached-worktree"
git worktree add --quiet --detach "$detached_worktree" HEAD
set +e
detached_local_output="$(
    env \
        GITHUB_ACTIONS=true \
        GITHUB_WORKFLOW=CI \
        GITHUB_JOB=fast-validation-preflight \
        GITHUB_REPOSITORY=Richards-LLC/cassy \
        GITHUB_EVENT_NAME=merge_group \
        GITHUB_REF=refs/heads/gh-readonly-queue/main/pr-123-abc \
        env -u GITHUB_ACTIONS \
        -u GITHUB_WORKFLOW \
        -u GITHUB_JOB \
        -u GITHUB_REPOSITORY \
        -u GITHUB_EVENT_NAME \
        -u GITHUB_REF \
        CLOCK_STATE="$tmp/detached-local-clock-state" CLOCK_STEP=0 \
        "$receipt" --repo-root "$detached_worktree"
)"
detached_local_status=$?
set -e
if [[ "$detached_local_status" -ne 0 ]]; then ok 'ambient CI cannot contaminate local detached checkout'; else bad 'ambient CI contaminated local detached checkout'; fi
expect_field "$detached_local_output" READINESS_MODE detached-rejected 'ambient-CI local detached readiness is labeled as rejected'

# The exact CI preflight identity is the only detached-checkout exception.
set +e
detached_ci_output="$(
    GITHUB_ACTIONS=true \
    GITHUB_WORKFLOW=CI \
    GITHUB_JOB=fast-validation-preflight \
    GITHUB_REPOSITORY=Richards-LLC/cassy \
    GITHUB_EVENT_NAME=merge_group \
    GITHUB_REF=refs/heads/gh-readonly-queue/main/pr-123-abc \
    CLOCK_STATE="$tmp/detached-ci-clock-state" CLOCK_STEP=0 \
    "$receipt" --repo-root "$detached_worktree"
)"
detached_ci_status=$?
set -e
if [[ "$detached_ci_status" -eq 0 ]]; then ok 'exact CI detached checkout passes'; else bad "exact CI detached checkout exited $detached_ci_status"; fi
expect_field "$detached_ci_output" READINESS_MODE github-actions-verification-detached 'CI detached readiness is labeled separately'
expect_field "$detached_ci_output" LOCAL_COMMIT_PUSH_READINESS_EXIT_STATUS 0 'CI detached readiness completes the verification checks'

# Remove the exact validated temporary branch and prove its ref is gone. The
# EXIT trap retains the same cleanup for failures before this assertion.
validated_branch="$named_branch"
git worktree remove --force "$named_worktree" >/dev/null
named_worktree=""
if git branch -D "$validated_branch" >/dev/null 2>&1; then
    named_branch=""
else
    bad 'temporary named branch cleanup failed'
fi
if git show-ref --verify --quiet "refs/heads/$validated_branch"; then
    bad 'temporary named branch ref leaked'
else
    ok 'temporary named branch ref is removed'
fi

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0

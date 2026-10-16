#!/usr/bin/env bash
# Deterministic self-test for scripts/release-latency-receipt.sh.
#
# The latency number is the claim GH #449 is closed on, so the script that
# produces it must be provably unable to flatter a release: it measures from
# the FIRST matching tag or main run (not a rerun), and it records an overrun
# without blocking the remaining work for an already-published release.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
receipt="$script_dir/release-latency-receipt.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/artifacts"
export CAS_RELEASE_ARTIFACTS_ROOT="$tmp/artifacts"
unset CAS_RELEASE_TRAIN_RUN_DIR CAS_RELEASE_RUN_DIR

pass=0
fail=0

ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }

expect_field() {
    local output="$1" field="$2" expected="$3" label="$4" actual
    actual="$(grep -m1 "^$field=" <<<"$output" | cut -d= -f2-)"
    if [[ "$actual" == "$expected" ]]; then ok "$label"; else
        bad "$label (expected $field=$expected; got $actual)"
    fi
}

cat >"$tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
case "$1 $2" in
  "release view")
    if [[ -z "${FAKE_PUBLISHED_AT:-}" ]]; then exit 1; fi
    printf '%s\n' "$FAKE_PUBLISHED_AT"
    ;;
  "api repos"*)
    printf '%s\n' "$*" >>"${FAKE_GH_LOG:-/dev/null}"
    case "$2" in
      */commits/*/pulls)
        cat "${FAKE_PR_JSON:?}"
        ;;
      */commits/*)
        printf '%s\n' "${FAKE_RELEASE_SHA:-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}"
        ;;
      *event=merge_group*)
        cat "${FAKE_MERGE_GROUP_RUNS_JSON:-${FAKE_EMPTY_RUNS_JSON:?}}"
        ;;
      *branch=main*)
        cat "${FAKE_MAIN_RUNS_JSON:-${FAKE_RUNS_JSON:?}}"
        ;;
      *)
        cat "${FAKE_RUNS_JSON:?}"
        ;;
    esac
    ;;
  *) echo "unexpected fake gh invocation: $*" >&2; exit 2 ;;
esac
EOF
chmod +x "$tmp/bin/gh"
export GH_BIN="$tmp/bin/gh"
export RELEASE_REPO=Richards-LLC/cassy
export FAKE_GH_LOG="$tmp/gh.log"
export FAKE_RELEASE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
export FAKE_EMPTY_RUNS_JSON="$tmp/empty-runs.json"
printf '{"workflow_runs":[]}\n' >"$FAKE_EMPTY_RUNS_JSON"
cat >"$tmp/pr.json" <<EOF
[{"number":1206,"base":{"ref":"main"},"merged_at":"2026-10-15T15:47:18Z","merge_commit_sha":"$FAKE_RELEASE_SHA"}]
EOF
export FAKE_PR_JSON="$tmp/pr.json"

# Two runs for the tag: the original push and a later rerun. Measuring from
# the rerun would understate the latency an operator actually experienced.
cat >"$tmp/runs.json" <<'EOF'
{"workflow_runs":[
  {"id":999,"created_at":"2026-08-20T12:30:00Z"},
  {"id":111,"created_at":"2026-08-20T12:00:00Z"}]}
EOF
export FAKE_RUNS_JSON="$tmp/runs.json"

# 1. Fast publication inside the budget.
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" v3.4.0)"
expect_field "$out" PUBLISH_LATENCY_SECONDS 250 'latency is measured from the first run of the tag'
expect_field "$out" TAG_RUN_ID 111 'receipt names the original tag run, not a rerun'
expect_field "$out" PUBLISH_START_KIND tag-push 'legacy releases identify their tag-push endpoint'
expect_field "$out" PUBLISH_RUN_ID 111 'generic receipt names the original tag workflow run'
expect_field "$out" BUDGET_SECONDS 600 'default budget is the ten-minute target'
expect_field "$out" WITHIN_BUDGET true 'a fast release reports within budget'

# A current release is triggered by the version commit landing on main; the
# workflow creates the tag later. Resolve the tag to its commit, then locate
# the successful merge-queue CI run for the PR's merged tree.
empty_runs="$FAKE_EMPTY_RUNS_JSON"
main_runs="$tmp/main-runs.json"
cat >"$main_runs" <<'EOF'
{"workflow_runs":[
  {"id":700,"event":"push","head_branch":"main","head_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","created_at":"2026-10-15T15:40:00Z"},
  {"id":901,"event":"push","head_branch":"main","head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","created_at":"2026-10-15T15:47:21Z"},
  {"id":902,"event":"push","head_branch":"main","head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","created_at":"2026-10-15T15:55:00Z"}]}
EOF
merge_group_runs="$tmp/merge-group-runs.json"
cat >"$merge_group_runs" <<'EOF'
{"workflow_runs":[
  {"id":800,"name":"CI","event":"merge_group","status":"completed","conclusion":"success","head_branch":"gh-readonly-queue/main/pr-1206-wrong","head_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","updated_at":"2026-10-15T15:40:00Z"},
  {"id":801,"name":"Release Prebuild","event":"merge_group","status":"completed","conclusion":"success","head_branch":"gh-readonly-queue/main/pr-1206-sample","head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","updated_at":"2026-10-15T15:46:00Z"},
  {"id":802,"name":"CI","event":"merge_group","status":"completed","conclusion":"failure","head_branch":"gh-readonly-queue/main/pr-1206-sample","head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","updated_at":"2026-10-15T15:46:30Z"},
  {"id":803,"name":"CI","event":"merge_group","status":"completed","conclusion":"success","head_branch":"gh-readonly-queue/main/pr-1206-sample","head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","updated_at":"2026-10-15T15:47:08Z"},
  {"id":804,"name":"CI","event":"merge_group","status":"completed","conclusion":"success","head_branch":"gh-readonly-queue/main/pr-1206-sample","head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","updated_at":"2026-10-15T15:47:25Z"}]}
EOF
main_run_dir="$tmp/artifacts/v3.55.0-latency"
out="$(FAKE_RUNS_JSON="$empty_runs" FAKE_MAIN_RUNS_JSON="$main_runs" \
    FAKE_MERGE_GROUP_RUNS_JSON="$merge_group_runs" \
    FAKE_PUBLISHED_AT=2026-10-15T15:57:34Z \
    "$receipt" v3.55.0)"
expect_field "$out" PUBLISH_START_KIND gate-green 'main-push release uses authoritative merge-queue green'
expect_field "$out" PUBLISH_START_AT 2026-10-15T15:47:08Z 'green timing starts at the successful merge-queue CI completion'
expect_field "$out" GATE_GREEN_EPOCH 1792079228 'receipt records the authoritative gate-green epoch'
expect_field "$out" GATE_GREEN_RUN_ID 803 'receipt identifies the successful merge-queue CI run'
expect_field "$out" MAIN_LANDED_EPOCH 1792079238 'receipt separately records the PR main landing'
expect_field "$out" MAIN_PUSH_RUN_ID 901 'main-push receipt names the first exact-SHA run'
expect_field "$out" PUBLISH_RUN_ID 901 'generic receipt identifies the selected publication run'
expect_field "$out" PUBLISH_LATENCY_SECONDS 626 'green-to-published timing matches the v3.55 measured interval'
if grep -q '^TAG_PUSHED_AT=' <<<"$out" || grep -q '^TAG_RUN_ID=' <<<"$out"; then
    bad 'main-push time is not mislabeled as a tag push'
else
    ok 'main-push time is not mislabeled as a tag push'
fi
expect_field "$(cat "$main_run_dir/release-latency.receipt")" PUBLISH_LATENCY_SECONDS 626 \
    'standalone measurement writes the report receipt under the release artifact root'
if [[ "$(cat "$main_run_dir/gate.green.epoch")" == 1792079228 && \
      "$(cat "$main_run_dir/main.landed.epoch")" == 1792079238 ]]; then
    ok 'standalone measurement persists both authoritative endpoint epochs'
else
    bad 'standalone measurement did not persist the endpoint epochs'
fi
latency_draft="$tmp/main-latency-draft.md"
cat >"$latency_draft" <<'EOF'
```text
*Live on production — User — Cassy v3.55.0*
Was: timing was missing → Now: timing is recorded.
```

```text
• *User update* — Was: no timing. Now: the release start is shown.
```

```text
*Live on production — Dev — Cassy v3.55.0*
Was: timing was missing → Now: timing is recorded.
```

```text
• *Developer update* — Was: no timing. Now: the release start is shown.
```
EOF
python3 "$script_dir/release-train-announce.py" --record-latency v3.55.0 \
    "$main_run_dir/release-latency.receipt" "$latency_draft"
grep -qF 'Green to published: 626s' "$latency_draft" \
    && ok 'announcement draft labels authoritative gate-green timing accurately' \
    || bad 'announcement draft did not label gate-green timing'

# Older releases may not have a merge_group CI run. Use the merged PR time as
# an explicit main-to-published fallback and do not manufacture gate-green.
rm -f "$main_run_dir/gate.green.epoch"
out="$(FAKE_RUNS_JSON="$empty_runs" FAKE_MAIN_RUNS_JSON="$main_runs" \
    FAKE_MERGE_GROUP_RUNS_JSON="$empty_runs" \
    FAKE_PUBLISHED_AT=2026-10-15T15:57:34Z \
    "$receipt" v3.55.0)"
expect_field "$out" PUBLISH_START_KIND main-landed 'missing merge-queue evidence selects the main-landing fallback'
expect_field "$out" PUBLISH_START_AT 2026-10-15T15:47:18Z 'fallback endpoint is the merged PR timestamp'
expect_field "$out" MAIN_LANDED_EPOCH 1792079238 'fallback receipt records main landing epoch'
expect_field "$out" PUBLISH_LATENCY_SECONDS 616 'fallback matches the hand-measured main-to-published interval'
if grep -q '^GATE_GREEN_EPOCH=' <<<"$out"; then
    bad 'fallback leaves gate-green unset when merge-queue evidence is absent'
else
    ok 'fallback leaves gate-green unset when merge-queue evidence is absent'
fi
if [[ -e "$main_run_dir/gate.green.epoch" && ! -e "$main_run_dir/main.landed.epoch" ]]; then
    bad 'fallback wrote a gate-green artifact instead of main-landing evidence'
elif [[ ! -e "$main_run_dir/gate.green.epoch" && "$(cat "$main_run_dir/main.landed.epoch")" == 1792079238 ]]; then
    ok 'fallback persists only the main-landing endpoint file'
else
    bad 'fallback endpoint files disagree with the main-landing measurement'
fi
python3 "$script_dir/release-train-announce.py" --record-latency v3.55.0 \
    "$main_run_dir/release-latency.receipt" "$latency_draft"
grep -qF 'Main to published: 616s' "$latency_draft" \
    && ok 'announcement draft labels main-landing fallback accurately' \
    || bad 'announcement draft did not label main-landing timing'
legacy_receipt="$tmp/legacy-latency.receipt"
cat >"$legacy_receipt" <<'EOF'
TAG=v3.4.0
TAG_PUSHED_AT=2026-08-20T12:00:00Z
PUBLISHED_AT=2026-08-20T12:04:10Z
PUBLISH_LATENCY_SECONDS=250
BUDGET_SECONDS=600
WITHIN_BUDGET=true
INTERVENTIONS=0
EOF
python3 "$script_dir/release-train-announce.py" --record-latency v3.4.0 \
    "$legacy_receipt" "$latency_draft"
grep -qF 'Tag to published: 250s' "$latency_draft" \
    && ok 'announcement draft remains compatible with legacy tag receipts' \
    || bad 'legacy tag timing was not preserved'
grep -qF 'actions/workflows/release.yml/runs?branch=main&event=push&head_sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&per_page=100' "$FAKE_GH_LOG" \
    && ok 'main-push lookup uses the release workflow main/push endpoint' \
    || bad 'main-push lookup did not use the release workflow main/push endpoint'
grep -qF 'actions/runs?event=merge_group&head_sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&per_page=100' "$FAKE_GH_LOG" \
    && ok 'main-push lookup uses the merged release SHA to find merge-queue CI' \
    || bad 'main-push lookup did not query merge-queue CI by the release SHA'

# Release-train hand-off and intervention metrics come from the run directory,
# not from GitHub's tag workflow timestamps. A manual targeted gate is one
# intervention and names the canonical stage in BLOCKERS.
run_dir="$tmp/release-run"
mkdir -p "$run_dir"
cat >"$run_dir/interventions.log" <<'EOF'
2026-08-20T12:00:00Z subcommand=--cut stage=preflight caller=session-cut kind=internal resume=false blockers=none
2026-08-20T12:00:05Z subcommand=--gate stage=gate caller=session-operator kind=manual resume=false blockers=none
EOF
printf '100\n' >"$run_dir/gate.green.epoch"
printf '130\n' >"$run_dir/pipeline.start.epoch"
printf '200\n' >"$run_dir/pipeline.merged.epoch"
printf '245\n' >"$run_dir/publisher.start.epoch"
out="$(CAS_RELEASE_TRAIN_RUN_DIR="$run_dir" FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z \
    "$receipt" v3.4.0)"
expect_field "$out" INTERVENTIONS 1 'receipt counts manual interventions'
expect_field "$out" BLOCKERS gate 'receipt names the intervened stage'
expect_field "$out" GREEN_TO_PIPELINE_SECS 30 'receipt records green-to-pipeline hand-off delay'
expect_field "$out" MERGED_TO_PUBLISHER_SECS 45 'receipt records merged-to-publisher hand-off delay'
cat >>"$run_dir/interventions.log" <<'EOF'
2026-08-20T12:01:00Z subcommand=--cut stage=gate caller=session-operator kind=manual resume=true blockers=gate
2026-08-20T12:02:00Z subcommand=--cut stage=gate caller=session-operator kind=manual resume=true blockers=gate
2026-08-20T12:03:00Z subcommand=--cut stage=pipeline caller=session-operator kind=manual resume=true blockers=pipeline
EOF
out="$(CAS_RELEASE_TRAIN_RUN_DIR="$run_dir" FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z \
    "$receipt" v3.4.0)"
expect_field "$out" INTERVENTIONS 3 'resume interventions count each blocker once'
expect_field "$out" BLOCKERS gate,pipeline 'receipt preserves distinct blocker stages'

# Real rescued-release shape (captured from a production run): 15 internal rows, three resumed stages, five hand fixes.
fixture="$script_dir/tests/release-interventions-rescued"
cp "$fixture/interventions.txt" "$run_dir/interventions.log"
cp "$fixture/blockers.txt" "$run_dir/blockers.log"
cp "$fixture/supervisor-interventions.md" "$run_dir/supervisor-interventions.md"
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" v3.4.0 --run-dir "$run_dir")"
expect_field "$out" INTERVENTIONS 8 'v3.46 internal resumes and five hand fixes are counted'
expect_field "$out" BLOCKERS assemble,pipeline,publish 'v3.46 blocker stages remain distinct'
# A kind-only substitution on resumed rows must not change the count.
awk '/resume=true/ {sub("kind=internal", "kind=manual")} {print}' \
    "$fixture/interventions.txt" >"$run_dir/interventions.log"
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" v3.4.0 --run-dir "$run_dir")"
expect_field "$out" INTERVENTIONS 8 'resumed stage count is independent of caller kind'
rm "$run_dir/supervisor-interventions.md"
printf 'subcommand=--cut kind=internal resume=true blockers=assemble\n' >"$run_dir/interventions.log"
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" v3.4.0 --run-dir "$run_dir")"
expect_field "$out" INTERVENTIONS 1 'unresumed blocker stages do not inflate a recorded rescue'
rm "$run_dir/interventions.log"
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" v3.4.0 --run-dir "$run_dir")"
expect_field "$out" INTERVENTIONS 3 'blockers alone cannot report a clean release'
printf 'unrecognized legacy blocker\n' >"$run_dir/blockers.log"
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" v3.4.0 --run-dir "$run_dir")"
expect_field "$out" INTERVENTIONS 1 'noncanonical nonempty blocker evidence cannot report zero'
rm "$run_dir/blockers.log"
printf '# Hand fixes\n\n- first fix\n  continuation\n- second fix\n' >"$run_dir/supervisor-interventions.md"
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" v3.4.0 --run-dir "$run_dir")"
expect_field "$out" INTERVENTIONS 2 'hand fixes without invocation log count entries, not lines'

rm "$run_dir/supervisor-interventions.md"
printf 'subcommand=--cut stage=preflight kind=internal resume=false blockers=none\nsubcommand=--status stage=status kind=internal resume=false blockers=none\n' >"$run_dir/interventions.log"
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" v3.4.0 --run-dir "$run_dir")"
expect_field "$out" INTERVENTIONS 0 'ordinary internal dispatch and status reads remain zero'
expect_field "$out" BLOCKERS none 'ordinary internal calls do not invent blockers'

# End-to-end metrics (cas-a629): request and first cut to publication, and a
# run with two blockers priced from each block to its stage's completion.
published_epoch=1787227450  # 2026-08-20T12:04:10Z
e2e="$tmp/e2e-run"
mkdir -p "$e2e"
printf '%s\n' "$((published_epoch - 7200))" >"$e2e/release.request.epoch"
printf '%s\n' "$((published_epoch - 3600))" >"$e2e/cut.start.epoch"
event() { printf '%s\t%s\t%s\n' "$((published_epoch - $1))" "$2" "$3" >>"$e2e/stage-events.tsv"; }
event 3500 gate start
event 3400 gate blocked
event 3400 gate blocked        # the stage body and the cut loop both record it
event 3000 gate start
event 2800 gate done
event 2700 pipeline start
event 2600 pipeline blocked
event 1000 pipeline start
event 900 pipeline done
event 800 publish start
event 200 publish done
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" v3.4.0 --run-dir "$e2e")"
expect_field "$out" PUBLISH_LATENCY_SECONDS 250 'end-to-end metrics leave tag-to-published unchanged'
expect_field "$out" REQUEST_TO_PUBLISHED_SECS 7200 'request to published spans the whole wait'
expect_field "$out" REQUEST_SOURCE recorded 'a recorded request time is labelled recorded'
expect_field "$out" CUT_TO_PUBLISHED_SECS 3600 'cut to published starts at the first cut'
expect_field "$out" BLOCKER_COUNT 2 'two blocked stages are two blockers; a duplicate row is one'
expect_field "$out" BLOCKER_COSTS gate:600,pipeline:1700 'each blocker costs block to its stage completion'
expect_field "$out" BLOCKED_SECS 2300 'blocked time totals the blockers'
expect_field "$out" STAGE_SECS gate:200,pipeline:100,publish:600 'stage time is its last attempt'
rm "$e2e/release.request.epoch"
event 150 receipts start
event 100 receipts blocked
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" v3.4.0 --run-dir "$e2e")"
expect_field "$out" REQUEST_SOURCE cut-start 'without a request time the cut start is named as the source'
expect_field "$out" REQUEST_TO_PUBLISHED_SECS 3600 'request time falls back to the first cut'
expect_field "$out" BLOCKER_COSTS gate:600,pipeline:1700,receipts:100+ \
    'an unresolved blocker is priced to publication and marked open'
rm "$e2e/stage-events.tsv"
printf 'gate\npipeline\n' >"$e2e/blockers.log"
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" v3.4.0 --run-dir "$e2e")"
expect_field "$out" BLOCKER_COUNT 2 'a pre-event run counts blockers from blockers.log'
expect_field "$out" BLOCKER_COSTS unavailable 'a pre-event run cannot invent blocker costs'
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" v3.4.0 --run-dir "$tmp/no-such-run")"
expect_field "$out" CUT_TO_PUBLISHED_SECS unavailable 'a missing run directory reports unavailable'
if python3 "$script_dir/release-metrics.py" --normalize-request 2026-08-20T10:04:10Z | grep -qx "$((published_epoch - 7200))" \
    && ! python3 "$script_dir/release-metrics.py" --normalize-request 2999-01-01T00:00:00Z >/dev/null 2>&1 \
    && ! python3 "$script_dir/release-metrics.py" --normalize-request 2026-08-20T10:04:10 >/dev/null 2>&1; then
    ok 'request time accepts ISO 8601 UTC and refuses future or offset-less times'
else
    bad 'request time normalization'
fi

# 2. A slow published release must record the overrun and continue.
set +e
slow_out="$(FAKE_PUBLISHED_AT=2026-08-20T12:21:00Z "$receipt" v3.4.0 2>&1)"
slow_status=$?
set -e
if [[ "$slow_status" -eq 0 ]]; then
    ok 'an over-budget published release exits zero'
else
    bad 'an over-budget published release blocks completion'
fi
expect_field "$slow_out" WITHIN_BUDGET false 'overrun is retained in the receipt'
expect_field "$slow_out" PUBLISH_LATENCY_SECONDS 1260 'overrun retains the actual measurement'
grep -qF 'over the 600s budget' <<<"$slow_out" \
    && ok 'over-budget warning names the budget' \
    || bad 'over-budget warning does not name the budget'

# 3. An explicit budget is honoured.
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:21:00Z "$receipt" v3.4.0 --budget-seconds 1800)"
expect_field "$out" PUBLISH_LATENCY_SECONDS 1260 'explicit budget still reports the real latency'
expect_field "$out" WITHIN_BUDGET true 'an explicit wider budget passes'

# 4. An unpublished release cannot produce a latency receipt.
set +e
FAKE_PUBLISHED_AT= "$receipt" v3.4.0 >/dev/null 2>&1
unpublished_status=$?
set -e
if [[ "$unpublished_status" -eq 1 ]]; then
    ok 'an unpublished release fails instead of reporting a number'
else
    bad "an unpublished release exited $unpublished_status"
fi

# Missing or incoherent measurements still fail, with no success receipt.
for scenario in missing invalid reversed; do
    published=2026-08-20T12:04:10Z
    case "$scenario" in
        missing) printf '{"workflow_runs":[]}' >"$tmp/bad-runs.json" ;;
        invalid) printf '{"workflow_runs":[{"id":111,"created_at":"not-a-time"}]}' >"$tmp/bad-runs.json" ;;
        reversed) cp "$tmp/runs.json" "$tmp/bad-runs.json"; published=2026-08-20T11:59:59Z ;;
    esac
    if FAKE_RUNS_JSON="$tmp/bad-runs.json" FAKE_MAIN_RUNS_JSON="$empty_runs" FAKE_PUBLISHED_AT="$published" \
        "$receipt" v3.4.0 >"$tmp/bad.out" 2>"$tmp/bad.err"; then
        bad "$scenario latency measurement passed"
    elif [[ ! -s "$tmp/bad.out" ]]; then
        ok "$scenario latency measurement fails before emitting a receipt"
    else
        bad "$scenario latency measurement emitted a success receipt"
    fi
done

# 5. Argument validation.
for bad_args in "3.4.0" "v3.4.0 --budget-seconds"; do
    set +e
    # shellcheck disable=SC2086
    FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" $bad_args >/dev/null 2>&1
    status=$?
    set -e
    if [[ "$status" -eq 2 ]]; then
        ok "usage error for: $bad_args"
    else
        bad "expected usage exit 2 for: $bad_args (got $status)"
    fi
done

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0

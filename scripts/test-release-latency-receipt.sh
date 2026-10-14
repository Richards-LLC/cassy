#!/usr/bin/env bash
# Deterministic self-test for scripts/release-latency-receipt.sh.
#
# The latency number is the claim GH #449 is closed on, so the script that
# produces it must be provably unable to flatter a release: it measures from
# the FIRST run of the tag (not a rerun), and it records an overrun without blocking
# the remaining work for an already-published release.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
receipt="$script_dir/release-latency-receipt.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"

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
    cat "${FAKE_RUNS_JSON:?}"
    ;;
  *) echo "unexpected fake gh invocation: $*" >&2; exit 2 ;;
esac
EOF
chmod +x "$tmp/bin/gh"
export GH_BIN="$tmp/bin/gh"
export RELEASE_REPO=Richards-LLC/cassy

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
expect_field "$out" BUDGET_SECONDS 600 'default budget is the ten-minute target'
expect_field "$out" WITHIN_BUDGET true 'a fast release reports within budget'

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
    if FAKE_RUNS_JSON="$tmp/bad-runs.json" FAKE_PUBLISHED_AT="$published" \
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

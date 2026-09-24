#!/usr/bin/env bash
# Self-test for the user-journey tooling:
#   scripts/journeys-for-diff.py   catalog parsing, --check, diff → journeys
#   scripts/check-journey-evaluation.sh   the release-train prep gate
# Also checks that the real catalog validates and that prep still calls the gate.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
helper="$repo/scripts/journeys-for-diff.py"
gate="$repo/scripts/check-journey-evaluation.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fails=0
ok() { printf 'ok   %s\n' "$1"; }
bad() { printf 'FAIL %s\n' "$1"; fails=$((fails + 1)); }

# --- the real catalog -------------------------------------------------------
if out="$(cd "$repo" && python3 "$helper" --check 2>&1)"; then ok "real catalog validates: $out"; else bad "real catalog: $out"; fi
grep -q 'check-journey-evaluation.sh' "$repo/scripts/release-train.d/prep.sh" \
    && ok "release-train prep calls the journey gate" || bad "prep.sh no longer calls check-journey-evaluation.sh"

# --- fixture repository -----------------------------------------------------
fx="$tmp/repo"
mkdir -p "$fx/docs/qa/journey-evaluations" "$fx/hub-web/dist" "$fx/hub-web/src" "$fx/hub-web/e2e"
git -C "$fx" init -q -b main
git -C "$fx" config user.email fixture@example.test
git -C "$fx" config user.name fixture
cat >"$fx/docs/qa/journeys.md" <<'EOF'
# Catalog

## hub-web

- **Surface-wide:** `hub-web/src/main.ts`, `hub-web/dist/*`

### HUB-J1 · Reply

- **Entry:** open conversation
- **Goal:** my reply lands
- **Touches:** `hub-web/src/composer*.ts`
- **Suite:** `hub-web/e2e/reply.journey.ts`
- **Gaps:** none

**Steps**

1. Write and send — it sends

**Expected experience**

- it is quick

**Edge paths**

- refused

### HUB-J2 · Pair

- **Entry:** first open
- **Goal:** paired
- **Touches:** `hub-web/src/pair*.ts`
- **Suite:** not automated — needs a real relay
- **Gaps:** relay

**Steps**

1. Pair — pairs

**Expected experience**

- obvious

**Edge paths**

- expired code
EOF
printf 'test("HUB-J1 reply", async () => { await journey.stage("Write and send", async () => {}); });\n' >"$fx/hub-web/e2e/reply.journey.ts"
printf 'bundle v1\n' >"$fx/hub-web/dist/app.js"
printf 'main\n' >"$fx/hub-web/src/main.ts"
git -C "$fx" add -A && git -C "$fx" commit -q -m base && git -C "$fx" tag v1.0.0
base_sha="$(git -C "$fx" rev-parse HEAD)"

run_helper() { (cd "$fx" && python3 "$helper" "$@"); }
ids() { python3 -c 'import json,sys; print(",".join(j["id"]+":"+j["reason"] for j in json.load(sys.stdin)["journeys"]))'; }

if out="$(run_helper --check 2>&1)"; then ok "fixture catalog validates"; else bad "fixture catalog: $out"; fi
[[ "$(run_helper --paths hub-web/src/composer-markup.ts | ids)" == "HUB-J1:hub-web/src/composer*.ts" ]] \
    && ok "a touched file selects its journey" || bad "touched file mapping"
[[ "$(run_helper --paths hub-web/src/main.ts | ids)" == "HUB-J1:surface-wide:hub-web/src/main.ts,HUB-J2:surface-wide:hub-web/src/main.ts" ]] \
    && ok "a surface-wide file selects every journey of the surface" || bad "surface-wide mapping"
[[ "$(run_helper --paths hub-web/e2e/reply.journey.ts | ids)" == "HUB-J1:suite" ]] \
    && ok "a journey's own spec selects it" || bad "suite mapping"
[[ "$(run_helper --paths README.md | ids)" == "" ]] && ok "an unrelated path selects nothing" || bad "unrelated path"
printf 'x\n' >"$fx/hub-web/src/pairing-relay.ts"
git -C "$fx" add -A && git -C "$fx" commit -q -m pair-change
[[ "$(run_helper "$base_sha" | ids)" == "HUB-J2:hub-web/src/pair*.ts" ]] \
    && ok "diff mode maps base...HEAD" || bad "diff mode: $(run_helper "$base_sha")"

broken="$tmp/broken"
cp -r "$fx" "$broken"
sed -i '/\*\*Goal:\*\* paired/d; s/"Write and send"/"Type and send"/' "$broken/docs/qa/journeys.md" "$broken/hub-web/e2e/reply.journey.ts"
if out="$(cd "$broken" && python3 "$helper" --check 2>&1)"; then
    bad "--check accepted a broken catalog"
else
    [[ "$out" == *"HUB-J2: missing **Goal:**"* && "$out" == *"step 'Write and send' has no matching test.step"* ]] \
        && ok "--check names missing fields and drifted step titles" || bad "--check output: $out"
fi

# --- the release gate -------------------------------------------------------
gate_run() { (cd "$fx" && "$gate" "$fx" 2>&1); }

if out="$(gate_run)" && [[ "$out" == *"unchanged since v1.0.0"* ]]; then ok "gate: dist unchanged since the tag passes"; else bad "gate unchanged: $out"; fi

printf 'bundle v2\n' >"$fx/hub-web/dist/app.js"
git -C "$fx" add -A && git -C "$fx" commit -q -m ui-change
tree="$(git -C "$fx" rev-parse HEAD:hub-web/dist)"
if out="$(gate_run)"; then bad "gate passed a changed dist with no report"; else
    [[ "$out" == *"BLOCKER journey-evaluation"*"no committed report"* ]] && ok "gate: changed dist without a report blocks" || bad "gate no-report output: $out"
fi

report() { # <file> <tree> <blocking> <rows...>
    local file="$1" key="$2" blocking="$3"; shift 3
    { printf '# eval\n\n- hub_web_dist: %s\n- blocking_findings: %s\n\n| ID | Run |\n|---|---|\n' "$key" "$blocking"
      printf '%s\n' "$@"; } >"$fx/docs/qa/journey-evaluations/$file"
    git -C "$fx" add -A && git -C "$fx" commit -q -m "$file"
}
report stale.md "$(git -C "$fx" rev-parse "$base_sha:hub-web/dist")" 0 '| HUB-J1 | PASS | 0 |' '| HUB-J2 | PASS | 0 |'
if out="$(gate_run)"; then bad "gate accepted a report for another dist tree"; else ok "gate: a report for another dist tree does not count"; fi
report blocking.md "$tree" 1 '| HUB-J1 | PASS | 0 |' '| HUB-J2 | PASS | 0 |'
if out="$(gate_run)"; then bad "gate accepted blocking findings"; else
    [[ "$out" == *"blocking_findings is not 0"* ]] && ok "gate: blocking findings block" || bad "gate blocking output: $out"
fi
report missing.md "$tree" 0 '| HUB-J1 | PASS | 0 |' '| HUB-J2 | FAIL | 0 |'
if out="$(gate_run)"; then bad "gate accepted a FAIL row"; else
    [[ "$out" == *"HUB-J2 has no PASS row"* ]] && ok "gate: every catalog journey needs a PASS row" || bad "gate missing-row output: $out"
fi
report good.md "$tree" 0 '| HUB-J1 | PASS | 0 |' '| HUB-J2 | PASS | 0 |'
if out="$(gate_run)" && [[ "$out" == *"good.md covers hub-web/dist tree"* ]]; then ok "gate: a passing report for this dist tree passes"; else bad "gate good: $out"; fi

nodist="$tmp/nodist"
mkdir -p "$nodist" && git -C "$nodist" init -q -b main && printf 'x\n' >"$nodist/README.md"
git -C "$nodist" -c user.email=f@e.test -c user.name=f add -A && git -C "$nodist" -c user.email=f@e.test -c user.name=f commit -q -m x
if out="$("$gate" "$nodist" 2>&1)" && [[ "$out" == *"not applicable"* ]]; then ok "gate: a repo without hub-web/dist is not applicable"; else bad "gate nodist: $out"; fi

# --- server isolation (cas-00ad) ---------------------------------------------
# A journey run must never attach to another checkout's server: no reuse, no
# fixed shared default ports, and each checkout's pair stays in 20000–32767.
config="$repo/hub-web/playwright.config.ts"
if grep -q 'reuseExistingServer: false' "$config" && ! grep -q 'reuseExistingServer: !\|reuseExistingServer: true' "$config"; then
    ok "playwright config never reuses a running server"
else bad "playwright config may reuse another checkout's server"; fi
if grep -qE '\?\? *479[12]' "$config" "$repo/hub-web/e2e/journeys/serve-dist.mjs"; then
    bad "a fixed shared default port (4791/4792) is back"
else ok "no fixed shared default port"; fi
# Plain .mjs on purpose (cas-6942): the CI runner's Node predates
# --experimental-strip-types, and a swallowed stderr hid why this failed.
ports_err="$tmp/checkout-ports.err"
if ports="$(cd "$repo/hub-web" && node e2e/checkout-ports.mjs . 2>"$ports_err")"; then
    read -r fixtures journeys <<<"$ports"
    if [[ "$fixtures" =~ ^[0-9]+$ && "$journeys" =~ ^[0-9]+$ ]] \
        && (( fixtures >= 20000 && journeys <= 32767 && journeys == fixtures + 1 )); then
        ok "checkout ports $fixtures/$journeys sit in 20000–32767"
    else bad "checkout ports out of range: $ports"; fi
else bad "could not compute checkout ports with $(node --version 2>&1): $(cat "$ports_err")"; fi
strip_users="$(grep -rlE 'node +--experimental-strip-types' "$repo/scripts" "$repo/hub-web/package.json" "$repo/hub-web/e2e" "$repo/.github" 2>/dev/null | grep -v '/scripts/test-journeys\.sh$' || true)"
if [[ -n "$strip_users" ]]; then bad "journey tooling depends on --experimental-strip-types again: $strip_users"
else ok "journey tooling runs on Node without --experimental-strip-types"; fi

if [[ $fails -gt 0 ]]; then printf '%d failure(s)\n' "$fails"; exit 1; fi
printf 'all journey tooling checks passed\n'

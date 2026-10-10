#!/usr/bin/env bash
# Mechanical, fail-closed release train for an assembled epic worktree.
#
# The version bump, Cargo.lock refresh, CHANGELOG section, and Slack draft must
# already be committed on the source branch. This gate proves the tree before
# it enters the merge queue; it does not mutate release metadata. A receipt is
# printed even when one check fails so the failure can be pasted into the epic
# close note without relying on supervisor memory.
#
# Usage: scripts/release-gate.sh <version> [--reuse | --only <row,row>]
#        scripts/release-gate.sh --fast-rows [--base <ref>]

set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

failure_log_rel='cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md'
readonly -a gate_check_ids=(
    scratch-base epic-worktree-fresh epic-worktree-zig publish-toolchain failure-log ancestor-proxy-config assemble-stale-base
    version-literals ci-script-tests hub-web-tests release-binary-isa fixture-paths workspace-tests macos-check hub-web-dist-drift hub-web-visual-qa nextest doctests archive-mode
    snapshot-portability builtin-projections changelog-and-versions release-script release-notes-shell-injection
    procedure-guardrails working-tree test-targets markdown-lint test-shape test-env builtin-doc-hygiene
    journey-catalog builtin-skill-limits doctor-snapshot migration-registry ci-script-tests-changed
    fixture-paths-src
)

usage() {
    printf 'Usage: %s <version> [--reuse | --only <row,row>]\n' "$0"
    printf '       %s --fast-rows [--base <ref>]\n' "$0"
    printf '       %s --learn "<symptom>" "<cause>" "<check-id>" [--run-dir <dir> --evidence <file:line> ...]\n' "$0"
}

learn() {
    local symptom="$1" cause="$2" check_id="$3"
    local date entry path before mapping_dir='' known registered=false
    local -a mapping_rows=()
    shift 3
    while (($#)); do
        case "$1" in
            --run-dir) [[ $# -ge 2 ]] || return 2; mapping_dir="$2"; shift 2 ;;
            --evidence) [[ $# -ge 2 ]] || return 2; mapping_rows+=("$2"); shift 2 ;;
            *) printf 'error: unknown --learn option %s\n' "$1" >&2; return 2 ;;
        esac
    done
    if [[ -n "$mapping_dir" || ${#mapping_rows[@]} -gt 0 ]]; then
        [[ -n "$mapping_dir" && ${#mapping_rows[@]} -gt 0 ]] || {
            printf 'error: --run-dir and --evidence are required together\n' >&2; return 2;
        }
        for known in "${gate_check_ids[@]}"; do
            [[ "$check_id" == "$known" ]] && registered=true
        done
        "$registered" || { printf 'error: learned evidence needs an executable gate row: %s\n' "$check_id" >&2; return 2; }
        python3 "$repo_root/scripts/release-learning.py" --validate-map "$repo_root" \
            "$mapping_dir" "$check_id" "${mapping_rows[@]}" || return $?
    fi
    [[ "$symptom" != *$'\n'* && "$cause" != *$'\n'* ]] || {
        printf 'error: --learn values must be single-line strings\n' >&2
        return 2
    }
    [[ "$check_id" =~ ^(manual:)?[a-z0-9-]+$ ]] || {
        printf 'error: invalid check id %s\n' "$check_id" >&2
        return 2
    }
    date="$(date -u +%F)"
    entry="- $date — **$check_id** — Symptom: $symptom Root cause: $cause Release: operator-reported."
    for path in "$failure_log_rel"; do
        [[ -f "$path" ]] || {
            printf 'error: missing failure log %s\n' "$path" >&2
            return 1
        }
    done
    for path in "$failure_log_rel"; do
        before="$(mktemp)"
        cp "$path" "$before"
        printf '%s\n' "$entry" >>"$path"
        printf '%s\n' "--- $path"
        if ! diff -u "$before" "$path"; then
            :
        fi
        rm -f "$before"
    done
    if [[ -n "$mapping_dir" ]]; then
        python3 "$repo_root/scripts/release-learning.py" --map "$repo_root" \
            "$mapping_dir" "$check_id" "${mapping_rows[@]}" || return $?
    fi
    if [[ -x "$repo_root/scripts/gen-builtin-reference-history.sh" ]]; then
        "$repo_root/scripts/gen-builtin-reference-history.sh"
        printf 'Regenerated builtin reference history after --learn; commit the ledger before starting a gate.\n'
    fi
    printf 'Learned release failure in the failure log (one copy serves every harness). Add or extend check %s and its fixture self-test in this same commit; record the same text with memory action=remember tags=release before retrying.\n' "$check_id"
}

if [[ "${1:-}" == '--learn' ]]; then
    [[ "$#" -ge 4 ]] || {
        usage >&2
        exit 2
    }
    shift
    learn "$@"
    exit $?
fi

# The guardian must re-execute the requested mode/base, before parsing rewrites argv.
gate_original_args=("$@")

# Lane admission uses the current checked-in version, before release prep.
# An explicit base limits docs lint to this lane's merged delta. Without it,
# HEAD^ is the push delta; a root commit compares against the empty tree.
fast_rows=false
fast_base=''
if [[ "${1:-}" == --fast-rows ]]; then
    if [[ "$#" -ne 1 && ! ( "$#" -eq 3 && "$2" == --base && -n "$3" ) ]]; then
        usage >&2
        exit 2
    fi
    fast_rows=true
    fast_base="${3:-}"
    # A force-pushed branch's push event names the replaced tip as `before`.
    # That commit is not fetched and may no longer exist, so a missing base
    # falls back to merge-base(HEAD, default branch) instead of failing every
    # row that diffs against it. ZERO_BASE_REF names the default branch in CI.
    if [[ -n "$fast_base" ]] && ! git rev-parse --verify --quiet "$fast_base^{commit}" >/dev/null; then
        fallback_ref="${ZERO_BASE_REF:-origin/main}"
        if ! git rev-parse --verify --quiet "$fallback_ref^{commit}" >/dev/null \
            && [[ "$fallback_ref" == origin/* ]]; then
            GIT_TERMINAL_PROMPT=0 git fetch --quiet --no-tags origin \
                "+refs/heads/${fallback_ref#origin/}:refs/remotes/$fallback_ref" 2>/dev/null || true
        fi
        if fallback_base="$(git merge-base HEAD "$fallback_ref" 2>/dev/null)"; then
            printf 'fast rows: base %s is unavailable (force-push?); using merge-base with %s (%s)\n' \
                "$fast_base" "$fallback_ref" "$fallback_base"
            fast_base="$fallback_base"
        else
            printf 'fast rows: base %s and fallback %s are unavailable; comparing against HEAD^\n' \
                "$fast_base" "$fallback_ref"
            fast_base=''
        fi
    fi
    version="$(sed -n 's/^version = "\([^" ]*\)".*/\1/p' cas-cli/Cargo.toml | head -n1)"
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
        printf 'error: cannot read current release version from cas-cli/Cargo.toml\n' >&2
        exit 2
    }
    # Keep an explicit allowlist: adding a costly full-gate row cannot silently
    # add a build or a host-dependent release precondition to lane admission.
    integration_fast_rows="$(python3 scripts/release-integration-gates.py --fast-rows)" || exit $?
    set -- "$version" --only "$integration_fast_rows"
fi

if [[ "$#" -ne 1 && "$#" -ne 2 && "$#" -ne 3 ]] || [[ ! "${1:-}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    usage >&2
    exit 2
fi
if [[ "$#" -eq 3 && "$2" != '--only' ]]; then
    usage >&2
    exit 2
fi

if [[ "$#" -eq 2 && "$2" != '--reuse' ]]; then
    usage >&2
    exit 2
fi
version="$1"
only_rows="${3:-}"
reuse_rows=false
[[ "${2:-}" == --reuse ]] && reuse_rows=true
# cas-846f: a release assembled from main had no merge sweep, so its full gate
# must prove every row fresh. The train sets this; nothing here may reuse a row
# receipt, a sweep receipt or an assembly proof.
no_reuse=false
if [[ "${CAS_RELEASE_GATE_NO_REUSE:-}" == 1 ]]; then
    if "$reuse_rows"; then
        printf 'error: --reuse refused: this release requires a full gate with every row run fresh (CAS_RELEASE_GATE_NO_REUSE=1)\n' >&2
        exit 2
    fi
    no_reuse=true
fi
repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

# cas-fed5: portable host helpers (stat device, sha256sum, Cargo bin on PATH),
# so the gate runs on stock macOS without GNU coreutils shims.
gate_script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/release-portable.sh
source "$gate_script_dir/release-portable.sh"
# shellcheck source=scripts/release-test-env.sh
source "$gate_script_dir/release-test-env.sh"
release_portable_path_add_cargo_bin
release_portable_define_sha256sum

cargo_bin="${CARGO:-cargo}"
reference_history_script="${RELEASE_GATE_GEN_REFERENCE_HISTORY:-$repo_root/scripts/gen-builtin-reference-history.sh}"

# Scratch base for the two checks that must build OUTSIDE every checkout
# (archive-mode, snapshot-portability).
#
# The old default was $HOME/.cache/cas-release-gate. On any machine where the
# operator has a user-level ~/.cas store — which is every machine Cassy is
# actually installed on — that base has a .cas ancestor, so the gate's own
# assert_no_cas_ancestor refused those two rows and the release could not be
# cut until the operator exported CAS_RELEASE_GATE_HOME_DIR by hand. cas-4ccc
# fixed that for the self-test only; cas-c736 fixes the gate itself, so the
# variable becomes an override rather than a prerequisite. /var/tmp mirrors the
# merge-queue runner: outside every checkout and outside $HOME.
# cas-fed5: /Users/Shared on macOS, where /var/tmp is a Cassy disposable root.
scratch_base_default="$(release_portable_default_scratch_base)"
readonly scratch_base_default
if [[ -n "${CAS_RELEASE_GATE_HOME_DIR:-}" ]]; then
    scratch_base="$CAS_RELEASE_GATE_HOME_DIR"
    scratch_base_origin='CAS_RELEASE_GATE_HOME_DIR'
else
    scratch_base="$scratch_base_default"
    scratch_base_origin='default'
fi
readonly scratch_base scratch_base_origin
scratch_archive_history_file="$(dirname "$scratch_base")/.cas-release-gate-last-archive-size-bytes"
archive_size_file="${CAS_RELEASE_GATE_ARCHIVE_SIZE_FILE:-}"
readonly scratch_archive_history_file archive_size_file

# A guardian owns every large scratch path, forwards graceful signals to the
# entire child group, waits for it, then removes scratch and remap metadata.
if [[ -z "${CAS_RELEASE_GATE_SCRATCH_RUN_DIR:-}" ]]; then
    exec python3 "$repo_root/scripts/release_scratch.py" --repo "$repo_root" \
        --base "$scratch_base" guard -- bash "$repo_root/scripts/release-gate.sh" "${gate_original_args[@]}"
fi

# The gate IS the "slow CI environment" the `cas init` watchdog names.
#
# `cas init` aborts itself after CAS_INIT_TIMEOUT_SECS (default 300s) so a hang
# cannot squat a CPU core (cas-bf06). Dozens of tests spawn `cas init`, so every
# suite row here — nextest, doctests, archive-mode, snapshot-portability,
# builtin-projections — has that child in its tree. During the v3.15.1 gate,
# with three isolation re-runs compiling and six idle `cas serve` daemons
# resident, one of those children reached 300s and aborted; the archive-mode row
# failed on wall clock alone, and the same tree passed on the next attempt with
# the box quiet (cas-c0411).
#
# Raised rather than disabled, and only for the gate's own children: a wedged
# init still aborts here, just on a budget that suits a saturated machine. The
# ordinary `cas init` an operator runs keeps its 300s. The value sits above
# nextest's own per-test kill (slow-timeout 60s x terminate-after 10 = 600s), so
# inside a suite row nextest remains the thing that reports a hang, with the
# test's name attached.
readonly gate_init_timeout_secs_default=900
if [[ -n "${CAS_INIT_TIMEOUT_SECS:-}" ]]; then
    init_timeout_origin='CAS_INIT_TIMEOUT_SECS'
else
    CAS_INIT_TIMEOUT_SECS="$gate_init_timeout_secs_default"
    init_timeout_origin='release-gate'
fi
export CAS_INIT_TIMEOUT_SECS
readonly init_timeout_origin

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/cas-release-gate.XXXXXX")"
register_scratch() {
    python3 "$repo_root/scripts/release_scratch.py" --owner-dir "$CAS_RELEASE_GATE_SCRATCH_RUN_DIR" \
        --path "$1" register
}
register_scratch "$tmp_dir"
release_test_home="$tmp_dir/test-home"
mkdir -p "$release_test_home"
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP
# The guardian removes registered tmp_dir only after descendants are reaped.

# The train supplies a unique attempt directory. Successful logs survive just
# like failures; the temporary fallback remains useful for direct diagnostics.
row_log_dir="${CAS_RELEASE_GATE_LOG_DIR:-$tmp_dir/rows}"
mkdir -p "$row_log_dir"
rm -f "$row_log_dir/compile-timing.tsv" "$row_log_dir/memory-admission.json"
rm -f "$row_log_dir/compile-memory.jsonl" "$row_log_dir/link-rss.jsonl"
printf 'row\tstarted_utc\tended_utc\twall_s\tuser_s\tsystem_s\tstatus\tsource_sha\n' >"$row_log_dir/timing.tsv"
cache_dir="${CAS_RELEASE_GATE_CACHE_DIR:-}"
[[ "$no_reuse" == false ]] || cache_dir=''
# The train owns durable row evidence; unchanged inputs reuse it automatically.
[[ -z "$cache_dir" || -n "$only_rows" ]] || reuse_rows=true
cache_head="$(git rev-parse HEAD)"
gate_implementation="$(release_portable_realpath "${BASH_SOURCE[0]}")"
cache_toolchain=''
if [[ ( -n "$cache_dir" || "$reuse_rows" == true ) && -z "$only_rows" ]]; then
    [[ -z "$cache_dir" ]] || mkdir -p "$cache_dir"
    if ! cache_toolchain="$( { "$cargo_bin" --version && "$cargo_bin" nextest --version &&
        rustc -Vv && node --version && "${NPM:-npm}" --version; } 2>&1 | sha256sum | cut -d' ' -f1)"; then
        # Unknown tool identity is a cache miss, never a reason to skip tests.
        cache_dir=''
    fi
fi
failures=()

row_selected() {
    local name="$1"
    if [[ "$fast_rows" == true && "$name" == test-shape && ! -f scripts/check-test-shape.py ]]; then
        return 1
    fi
    [[ -z "$only_rows" || ",$only_rows," == *",$name,"* ]]
}

print_result() {
    local status="$1" name="$2" command="$3"
    printf '%s %s — %s\n' "$status" "$name" "$command"
}

# Explicit dependency boundary. Live checks and any unknown future row never
# reuse evidence. Web tests are independent of Rust; Cargo tests conservatively
# depend on the code-input projection, retaining embedded documentation. The
# assembly sweep uses the same fields, but stores its receipt in the shared
# merge-sweep directory because its detached checkout is not the release
# worktree that consumes the receipt.
cache_environment() {
    python3 -c '
import hashlib, os

def ignored(name):
    return name in {
        "_", "SHLVL", "CAS_FACTORY_SESSION", "CAS_AGENT_ROLE",
        "CAS_AGENT_NAME", "CAS_SUPERVISOR_NAME", "CAS_AGENT_ID",
        "CAS_RELEASE_GATE_LOG_DIR", "CAS_RELEASE_GATE_ARCHIVE_SIZE_FILE",
        "CAS_RELEASE_GATE_CACHE_DIR", "CAS_RELEASE_GATE_SWEEP_CACHE_DIR",
        "CAS_RELEASE_GATE_HOME_DIR", "CAS_RELEASE_GATE_SCRATCH_RUN_DIR",
        "CAS_RELEASE_GATE_SCRATCH_LEASE_FDS",
    } or name.startswith("CAS_RELEASE_TRAIN_")

material = "".join(
    f"{key}={value}\n"
    for key, value in sorted(os.environ.items())
    if not ignored(key)
)
print(hashlib.sha256(material.encode()).hexdigest())'
}

cache_input_hash() {
    local name="$1"
    case "$name" in
        hub-web-tests|hub-web-visual-qa|hub-web-dist-drift)
            git ls-tree -r HEAD -- hub-web scripts .github | sha256sum | cut -d' ' -f1
            ;;
        release-binary-isa)
            # Assembly masks release-version churn; this artifact proof also
            # binds the exact locked dependency graph, including member versions.
            {
                git rev-parse HEAD:Cargo.lock || return 1
                python3 "$repo_root/scripts/assembly-proof.py" input "$repo_root" || return 1
            } | sha256sum | cut -d' ' -f1
            ;;
        fixture-paths|workspace-tests|macos-check|nextest|doctests|archive-mode|snapshot-portability)
            if [[ -f "$repo_root/scripts/assembly-proof.py" ]]; then
                python3 "$repo_root/scripts/assembly-proof.py" input "$repo_root"
            else
                git rev-parse "$cache_head^{tree}"
            fi
            ;;
        *)
            return 1
            ;;
    esac
}

cache_git_common_dir="$(git rev-parse --path-format=absolute --git-common-dir)"
cache_checkout_identity="$(printf '%s\n' "$cache_git_common_dir" | sha256sum | cut -d' ' -f1)"
cache_implementation_digest="$(sha256sum "$gate_implementation" | cut -d' ' -f1)"
cache_repository_root="$(dirname "$cache_git_common_dir")"
sweep_cache_dir="${CAS_RELEASE_GATE_SWEEP_CACHE_DIR:-$cache_repository_root/.cas/merge-sweeps/row-cache}"
sweep_receipt="${CAS_RELEASE_GATE_SWEEP_RECEIPT:-$cache_repository_root/.cas/merge-sweeps/integration.json}"

assembly_sweep_green_for_head() {
    [[ -s "$sweep_receipt" ]] || return 1
    python3 - "$sweep_receipt" "$cache_head" <<'PY'
import json
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as stream:
        receipt = json.load(stream)
except (OSError, ValueError):
    raise SystemExit(1)
raise SystemExit(0 if receipt.get("status") == "PASSED" and receipt.get("tip") == sys.argv[2] else 1)
PY
}

row_receipt_valid() {
    local path="$1" expected_key="$2" expected_sha="$3" expected_input="$4"
    local expected_env="$5" expected_toolchain="$6" expected_checkout="$7" expected_implementation="$8"
    local receipt_key receipt_sha receipt_epoch receipt_status receipt_checkout receipt_input receipt_env receipt_toolchain receipt_implementation
    read -r receipt_key receipt_sha receipt_epoch receipt_status receipt_checkout receipt_input receipt_env receipt_toolchain receipt_implementation <"$path" || return 1
    [[ "$receipt_key" == "$expected_key" && "$receipt_sha" =~ ^[0-9a-f]{40}$ \
        && "$receipt_epoch" =~ ^[0-9]{10}$ && "$receipt_status" == PASS ]] || return 1
    # Gate-created v1 receipts only have the first four fields. They remain
    # valid for one migration cycle; newly-written receipts carry all fields
    # and are required to prove the assembly handoff.
    if [[ -z "${receipt_checkout:-}" && -z "${receipt_input:-}" \
        && -z "${receipt_env:-}" && -z "${receipt_toolchain:-}" \
        && -z "${receipt_implementation:-}" ]]; then
        return 0
    fi
    [[ "$receipt_checkout" == "$expected_checkout" \
        && "$receipt_input" == "$expected_input" \
        && "$receipt_env" == "$expected_env" \
        && "$receipt_toolchain" == "$expected_toolchain" \
        && "$receipt_implementation" == "$expected_implementation" ]]
}

row_cache_key() {
    local name="$1" env_fingerprint input_hash artifact_toolchain=''
    [[ ( -n "$cache_dir" || "$reuse_rows" == true ) && -z "$only_rows" ]] || return 1
    git diff --quiet HEAD || return 1
    [[ "$(git rev-parse HEAD)" == "$cache_head" ]] || return 1
    [[ -z "$(git ls-files --others --exclude-standard)" ]] || return 1
    local -a inputs=()
    case "$name" in
        hub-web-tests|hub-web-visual-qa|hub-web-dist-drift) inputs=(hub-web scripts .github) ;;
        release-binary-isa|fixture-paths|workspace-tests|macos-check|nextest|doctests|archive-mode|snapshot-portability)
            inputs=(.) ;;
        *) return 1 ;;
    esac
    env_fingerprint="$(cache_environment)" || return 1
    input_hash="$(cache_input_hash "$name")" || return 1
    if [[ "$name" == release-binary-isa ]]; then
        artifact_toolchain="$(release_binary_isa_toolchain)" || return 1
    fi
    {
        printf '%s\n' row-cache-v2 "$name" "$cache_checkout_identity" "$input_hash" \
            "$env_fingerprint" "$cache_toolchain" "$cache_implementation_digest"
        [[ "$name" != release-binary-isa ]] || printf '%s\n' "$artifact_toolchain"
    } | sha256sum | cut -d' ' -f1
}

run_check() {
    local name="$1" command="$2" function_name="$3" log status=0
    local started ended wall user system key='' source_sha="$cache_head"
    local receipt_key='' receipt_sha='' receipt_epoch='' receipt_status='' now receipt_path='' receipt_origin=''
    local env_fingerprint input_hash
    row_selected "$name" || return 0
    log="$row_log_dir/$name.log"
    started="$(date -u +%FT%TZ)"
    # Assembly/recovery already proved the CI script tiers, the native suite
    # and the queue's archive runner in a plain clone of the same code input
    # (the proof refuses a PASS unless all three passed on its tree). Consume
    # that proof even on the first full gate (cas-398c: ci-script-tests too);
    # --only remains a fresh diagnostic and cannot consume authorization.
    if [[ -z "$only_rows" && "$no_reuse" == false && "$name" =~ ^(nextest|archive-mode|ci-script-tests)$ \
        && -f "$repo_root/scripts/assembly-proof.py" ]]; then
        local assembly_pass=''
        if assembly_pass="$(python3 "$repo_root/scripts/assembly-proof.py" check "$repo_root" 2>&1)"; then
            source_sha="$(sed -n 's/.*source_sha=\([0-9a-f]*\).*/\1/p' <<<"$assembly_pass")"
            print_result PASS "$name" "$command"
            printf '  reused %s\n' "$assembly_pass"
            printf '%s\n' "$assembly_pass" >"$log"
            printf '%s\t%s\t%s\t0\t0\t0\tREUSED\t%s\n' "$name" "$started" "$started" "$source_sha" >>"$row_log_dir/timing.tsv"
            if [[ "$name" == archive-mode && -n "$archive_size_file" ]]; then
                mkdir -p "$(dirname "$archive_size_file")"
                sed -n 's/.*archive_size_bytes=\([0-9]*\).*/\1/p' <<<"$assembly_pass" >"$archive_size_file"
            fi
            return 0
        fi
        printf '  %s\n' "$assembly_pass"
    elif [[ -z "$only_rows" && "$no_reuse" == true && "$name" =~ ^(nextest|archive-mode|ci-script-tests)$ ]]; then
        printf '  MISS assembly key=implementation reason=full_gate_required\n'
    elif [[ -z "$only_rows" && "$name" =~ ^(nextest|archive-mode|ci-script-tests)$ ]]; then
        printf '  MISS assembly key=implementation reason=helper_missing\n'
    fi
    key="$(row_cache_key "$name" || true)"
    env_fingerprint="$(cache_environment)"
    input_hash="$(cache_input_hash "$name" || true)"
    if "$reuse_rows" && [[ -n "$key" ]]; then
        receipt_path=''
        receipt_origin=''
        if [[ -n "$cache_dir" && -f "$cache_dir/$name.$key" ]]; then
            receipt_path="$cache_dir/$name.$key"
            receipt_origin='gate'
        elif assembly_sweep_green_for_head && [[ -f "$sweep_cache_dir/$name.$key" ]]; then
            receipt_path="$sweep_cache_dir/$name.$key"
            receipt_origin='assembly sweep'
        fi
        now="$(date +%s)"
        if [[ -n "$receipt_path" ]] && row_receipt_valid "$receipt_path" "$key" "$cache_head" \
            "$input_hash" "$env_fingerprint" "$cache_toolchain" "$cache_checkout_identity" \
            "$cache_implementation_digest"; then
            read -r receipt_key receipt_sha receipt_epoch receipt_status _ <"$receipt_path" || true
        fi
        if [[ -n "$receipt_path" && "$receipt_key" == "$key" && "$receipt_sha" =~ ^[0-9a-f]{40}$ \
            && "$receipt_epoch" =~ ^[0-9]{10}$ && "$receipt_status" == PASS ]] \
            && (( now >= receipt_epoch && now - receipt_epoch <= 86400 )); then
            print_result PASS "$name" "$command"
            printf '  reused PASS from %s (%s)\n' "$receipt_sha" "$receipt_origin"
            printf 'Reused PASS key=%s source_sha=%s epoch=%s source=%s\n' "$key" "$receipt_sha" "$receipt_epoch" "$receipt_origin" >"$log"
            printf '%s\t%s\t%s\t0\t0\t0\tREUSED\t%s\n' "$name" "$started" "$started" "$receipt_sha" >>"$row_log_dir/timing.tsv"
            return 0
        fi
    fi
    # Bash time measures shell functions and their children without moving
    # checks into subshells (the Zig resolver must export into later rows).
    local LC_NUMERIC=C TIMEFORMAT='%R %U %S'
    if { time "$function_name" >"$log" 2>&1; } 2>"$tmp_dir/$name.time"; then
        print_result PASS "$name" "$command"
        if [[ -n "$key" ]] && git diff --quiet HEAD \
            && [[ "$(git rev-parse HEAD)" == "$cache_head" ]] \
            && [[ -z "$(git ls-files --others --exclude-standard)" ]]; then
            printf '%s %s %s PASS %s %s %s %s %s\n' "$key" "$cache_head" "$(date +%s)" \
                "$cache_checkout_identity" "$input_hash" "$env_fingerprint" \
                "$cache_toolchain" "$cache_implementation_digest" >"$cache_dir/$name.$key.tmp.$$"
            mv "$cache_dir/$name.$key.tmp.$$" "$cache_dir/$name.$key"
        fi
    else
        status=$?
        print_result FAIL "$name" "$command"
        failures+=("$name")
        [[ -z "$key" ]] || rm -f "$cache_dir/$name.$key"
        if [[ -s "$log" ]]; then
            sed 's/^/  | /' "$log" | tail -20
        fi
        printf '  | exit status: %s\n' "$status"
    fi
    ended="$(date -u +%FT%TZ)"
    read -r wall user system <"$tmp_dir/$name.time"
    printf '  interval: %s to %s\n' "$started" "$ended"
    printf '  timing: wall=%ss user=%ss system=%ss\n' "$wall" "$user" "$system"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$name" "$started" "$ended" "$wall" "$user" "$system" "$status" "$source_sha" >>"$row_log_dir/timing.tsv"
}

is_gate_check_id() {
    local candidate="$1" known
    for known in "${gate_check_ids[@]}"; do
        [[ "$candidate" == "$known" ]] && return 0
    done
    return 1
}

if [[ "$#" -eq 3 && -z "$only_rows" ]]; then
    printf 'error: --only requires at least one release-gate row\n' >&2
    exit 2
fi
if [[ -n "$only_rows" ]]; then
    IFS=',' read -r -a requested_rows <<<"$only_rows"
    for requested in "${requested_rows[@]}"; do
        if [[ -z "$requested" ]] || ! is_gate_check_id "$requested"; then
            printf 'error: unknown --only release-gate row %s\n' "${requested:-<empty>}" >&2
            exit 2
        fi
    done
fi
selected_rows_summary=''
if [[ -n "$only_rows" ]]; then
    for requested in "${gate_check_ids[@]}"; do
        if row_selected "$requested"; then
            selected_rows_summary="${selected_rows_summary:+$selected_rows_summary,}$requested"
        fi
    done
fi
printf '%s\n' "${selected_rows_summary//,/$'\n'}" >"$row_log_dir/plan.txt"

# This is deliberately the first receipt row. It must reject a bad scratch
# location in seconds, before a Cargo process can spend a gate cycle filling the
# wrong filesystem. Test-only numeric seams make mount and capacity failures
# deterministic without requiring privileged fixture mounts.
check_scratch_base() {
    local parent probe_file parent_writable checkout_device scratch_device
    local free_bytes free_kib last_archive_bytes required_bytes
    parent="$(dirname "$scratch_base")"
    [[ -d "$parent" ]] || {
        printf 'scratch-base: parent %s does not exist; create it before the gate\n' "$parent"
        return 1
    }
    parent_writable="${CAS_RELEASE_GATE_PARENT_WRITABLE:-}"
    if [[ -z "$parent_writable" ]]; then
        probe_file="$(mktemp "$parent/.cas-release-gate-write.XXXXXX" 2>/dev/null || true)"
        if [[ -n "$probe_file" ]]; then
            rm -f "$probe_file"
            parent_writable=1
        else
            parent_writable=0
        fi
    fi
    if [[ "$parent_writable" != 1 ]]; then
        printf 'scratch-base: parent %s is not writable; mktemp creates siblings of %s there\n' \
            "$parent" "$scratch_base"
        return 1
    fi

    assert_no_cas_ancestor "$scratch_base" || return 1
    checkout_device="${CAS_RELEASE_GATE_CHECKOUT_DEVICE:-$(release_portable_stat_device "$repo_root" || true)}"
    scratch_device="${CAS_RELEASE_GATE_SCRATCH_DEVICE:-$(release_portable_stat_device "$parent" || true)}"
    if [[ -z "$checkout_device" || -z "$scratch_device" || "$checkout_device" != "$scratch_device" ]]; then
        printf 'scratch-base: filesystem boundary: checkout device=%s scratch-parent device=%s; use a base on the checkout mount\n' \
            "${checkout_device:-unknown}" "${scratch_device:-unknown}"
        return 1
    fi

    last_archive_bytes="${CAS_RELEASE_GATE_LAST_ARCHIVE_BYTES:-}"
    if [[ -z "$last_archive_bytes" && -s "$scratch_archive_history_file" ]]; then
        last_archive_bytes="$(tr -d '[:space:]' <"$scratch_archive_history_file")"
        [[ "$last_archive_bytes" =~ ^[0-9]+$ ]] || {
            printf 'scratch-base: invalid prior archive-size receipt %s\n' "$scratch_archive_history_file"
            return 1
        }
    fi
    free_bytes="${CAS_RELEASE_GATE_FREE_BYTES:-}"
    if [[ -z "$free_bytes" ]]; then
        free_kib="$(df -Pk "$parent" 2>/dev/null | awk 'NR == 2 { print $4 }')"
        [[ "$free_kib" =~ ^[0-9]+$ ]] && free_bytes=$((free_kib * 1024))
    fi
    [[ "$free_bytes" =~ ^[0-9]+$ ]] || {
        printf 'scratch-base: could not measure free bytes on %s\n' "$parent"
        return 1
    }
    if [[ "$last_archive_bytes" =~ ^[0-9]+$ && "$last_archive_bytes" -gt 0 ]]; then
        required_bytes=$((last_archive_bytes * 2))
        if (( free_bytes < required_bytes )); then
            printf 'scratch-base: %s bytes free, need at least %s (2x last archive %s)\n' \
                "$free_bytes" "$required_bytes" "$last_archive_bytes"
            return 1
        fi
        printf 'scratch-base: parent writable; same device %s; %s bytes free >= 2x last archive %s\n' \
            "$scratch_device" "$free_bytes" "$last_archive_bytes"
    else
        printf 'scratch-base: parent writable; same device %s; %s bytes free; no prior archive size recorded\n' \
            "$scratch_device" "$free_bytes"
    fi
}

# `worktree_merge` advances the shared epic ref, but a second worktree with
# that branch checked out keeps its old index and files until it is reset. The
# gate must never vouch for that stale tree. `CAS_RELEASE_EPIC_REF` is useful
# for detached gate worktrees; the train's branch setting and the current
# symbolic branch are the normal sources.
check_epic_worktree_fresh() {
    local claimed branch epic_ref expected head dirty
    claimed="${CAS_RELEASE_EPIC_REF:-${CAS_RELEASE_TRAIN_BRANCH:-}}"
    branch="$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
    if [[ -z "$claimed" ]]; then
        claimed="$branch"
    fi
    if [[ -z "$claimed" ]]; then
        printf 'epic-worktree-fresh: HEAD is detached and no epic ref was supplied\n'
        printf 'epic-worktree-fresh: reset command: git -C %q reset --hard HEAD\n' "$repo_root"
        return 1
    fi
    if [[ "$claimed" == refs/* || "$claimed" == HEAD || "$claimed" =~ ^[0-9a-fA-F]{7,40}$ ]]; then
        epic_ref="$claimed"
    else
        epic_ref="refs/heads/$claimed"
    fi
    expected="$(git rev-parse --verify "${epic_ref}^{commit}" 2>/dev/null || true)"
    head="$(git rev-parse --verify HEAD 2>/dev/null || true)"
    dirty="$(git status --porcelain)"
    if [[ -z "$expected" || -z "$head" || -n "$dirty" || "$head" != "$expected" ]]; then
        printf 'epic-worktree-fresh: refusing stale or dirty worktree (HEAD=%s ref=%s expected=%s)\n' \
            "${head:-missing}" "$epic_ref" "${expected:-missing}"
        printf 'epic-worktree-fresh: reset command: git -C %q reset --hard HEAD\n' "$repo_root"
        return 1
    fi
    printf 'epic-worktree-fresh: clean and current at %s (%s)\n' "$head" "$epic_ref"
}

# Zig is gitignored, so a newly-created epic worktree may not have the
# compiler even though the main checkout does. Resolve it before Cargo-backed
# rows run and export the absolute path for every child of this gate.
check_epic_worktree_zig() {
    local configured common_dir main_checkout candidate resolved
    configured="${ZIG:-}"
    if [[ -n "$configured" ]]; then
        [[ "$configured" = /* ]] || configured="$repo_root/$configured"
        if [[ -x "$configured" ]]; then
            resolved="$(cd "$(dirname "$configured")" && pwd -P)/$(basename "$configured")"
            export ZIG="$resolved"
            printf 'epic-worktree-zig: using executable ZIG=%s (environment)\n' "$ZIG"
            return 0
        fi
    fi

    candidate="$repo_root/.context/zig/zig"
    if [[ -x "$candidate" ]]; then
        export ZIG="$candidate"
        printf 'epic-worktree-zig: using executable ZIG=%s (epic worktree)\n' "$ZIG"
        return 0
    fi

    common_dir="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
    main_checkout="${common_dir%/.git}"
    [[ -n "$main_checkout" ]] || main_checkout="$repo_root"
    candidate="$main_checkout/.context/zig/zig"
    if [[ -x "$candidate" ]]; then
        export ZIG="$candidate"
        printf 'epic-worktree-zig: using executable ZIG=%s (main checkout)\n' "$ZIG"
        return 0
    fi

    printf 'epic-worktree-zig: no executable Zig compiler found in ZIG, %s, or %s\n' \
        "$repo_root/.context/zig/zig" "$main_checkout/.context/zig/zig"
    printf 'epic-worktree-zig: refusing the release; run ./scripts/bootstrap-zig.sh\n'
    return 1
}

check_failure_log() {
    local log="$failure_log_rel"
    local entry id
    local entries=0 enforced=0 manual=0 invalid=0
    [[ -f "$log" ]] || {
        printf 'failure-log: missing %s\n' "$log"
        return 1
    }
    while IFS= read -r entry || [[ -n "$entry" ]]; do
        [[ -z "$entry" ]] && continue
        entries=$((entries + 1))
        if [[ "$entry" == *manual:* ]]; then
            manual=$((manual + 1))
        elif [[ "$entry" =~ \*\*([a-z0-9-]+)\*\* ]]; then
            id="${BASH_REMATCH[1]}"
            if is_gate_check_id "$id"; then
                enforced=$((enforced + 1))
            else
                invalid=$((invalid + 1))
                printf 'failure-log: unknown executable check id %s\n' "$id"
            fi
        else
            invalid=$((invalid + 1))
            printf 'failure-log: entry has no **check-id** or manual: marker: %s\n' "$entry"
        fi
    done <"$log"
    printf 'failure-log: %d entries enforced; %d entries with no executable check (explicit manual markers); %d invalid\n' \
        "$enforced" "$manual" "$invalid"
    [[ "$entries" -gt 0 && "$invalid" -eq 0 ]]
}

check_assemble_stale_base() {
    local fixture="scripts/test-release-integration.py"
    [[ -f "$fixture" ]] || {
        printf 'assemble-stale-base: missing %s\n' "$fixture"
        return 1
    }
    # Recovery launches factory sweep children.  Keep the supervisor's
    # identity out of this process boundary so the executable fixture proves
    # the GH #901 regression cannot be hidden by the invoking shell.
    release_test_child env \
        python3 "$fixture"
}

# NUL-separated candidate files for the version-literals row: tracked files in
# a git checkout (gitignored build caches are skipped), every file otherwise.
version_literal_candidates() {
    local root
    if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        git ls-files -z -- "$@" 2>/dev/null
        return
    fi
    for root in "$@"; do
        [[ -d "$root" ]] || continue
        find "$root" -type f -print0
    done
}

check_version_literals() {
    local file hit found=false
    local -a roots=(cas-cli/src cas-cli/tests crates scripts)

    # Tracked files only: gitignored build caches (for example
    # crates/ghostty_vt_sys/zig/.zig-cache) embed absolute checkout paths, and
    # a worktree named after the release would otherwise fail this row.
    while IFS= read -r -d '' file; do
        [[ -f "$file" ]] || continue
        case "$file" in
            *.md|*/reference-history.json|*/failure-log.md|*/Cargo.toml|*/Cargo.lock) continue ;;
        esac
        hit="$(grep -nIF -- "$version" "$file" 2>/dev/null || true)"
        if [[ -n "$hit" ]]; then
            printf '%s\n' "$hit"
            found=true
        fi
    done < <(version_literal_candidates "${roots[@]}")

    if [[ "$found" == true ]]; then
        printf 'version-literals: source/test files contain %s; use env!("CARGO_PKG_VERSION") or a fixture value\n' "$version"
        return 1
    fi
}

# Runtime reads of the producer checkout through CARGO_MANIFEST_DIR pass on
# every build host and fail on the merge-queue shard runner, which has only the
# compiled test binary (cas-1f6e: a builtins unit test read docs/design copies
# that way; the archive row could not catch it because the producer path still
# existed here). The Rust guard below scans the embedded integration-test
# sources; this scan covers the src-side test modules the guard cannot embed,
# and the workspace crates' src and tests (cas-ae01: the merge queue failed
# on a cas-store src guard that read_dir'ed its own src at runtime).
# Matched: a manifest-relative Path/PathBuf, `.join`/`.parent` on one, a
# std::fs call or File::open on the env! value, a runtime `format!` of it, and
# binding it to a variable. Compile-time `include_str!(concat!(env!(
# "CARGO_MANIFEST_DIR"), ...))` and `include_bytes!` are shard-safe and are
# not matched.
readonly src_runtime_manifest_dir_pattern='Path::new\(env!\("CARGO_MANIFEST_DIR"\)\)|PathBuf::from\(env!\("CARGO_MANIFEST_DIR"\)\)|env!\("CARGO_MANIFEST_DIR"\)\)\.(join|parent)\(|(fs::[a-z_]+|File::open)\(\s*&?\s*(concat!\(\s*)?env!\("CARGO_MANIFEST_DIR"\)|format!\([^;]*env!\("CARGO_MANIFEST_DIR"\)|=\s*env!\("CARGO_MANIFEST_DIR"\)\s*;'
readonly -a src_runtime_manifest_dir_roots=(cas-cli/src crates/*/src crates/*/tests)
# cas::test_paths::workspace_root() is the one sanctioned runtime probe; its
# callers guard it with an explicit SKIP when the checkout is absent. The two
# crate tests are reviewed: an #[ignore]d live-traffic contract test, and a
# fallback reached only when no test_tui binary sits beside the test binary.
readonly -a src_runtime_manifest_dir_reviewed=(
    cas-cli/src/test_paths.rs
    crates/cas-mux/tests/claude_factory_contract_runtime.rs
    crates/cas-tui-test/tests/tui_e2e_test.rs
)

# Runtime manifest-dir reads in the given files, minus the reviewed ones.
src_runtime_manifest_dir_hits() {
    local -a files=() path reviewed
    for path in "$@"; do
        for reviewed in "${src_runtime_manifest_dir_reviewed[@]}"; do
            [[ "$path" == "$reviewed" ]] && continue 2
        done
        files+=("$path")
    done
    [[ "${#files[@]}" -gt 0 ]] || return 0
    grep -nHE "$src_runtime_manifest_dir_pattern" "${files[@]}" 2>/dev/null || true
}

check_src_runtime_manifest_dir_reads() {
    local hits
    local -a files=()
    mapfile -t files < <(find "${src_runtime_manifest_dir_roots[@]}" -name '*.rs' -type f 2>/dev/null | sort)
    hits="$(src_runtime_manifest_dir_hits "${files[@]}")"
    if [[ -n "$hits" ]]; then
        printf 'fixture-paths: test code reads the producer checkout at runtime; embed the file with include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/..")) or use cas::test_paths::workspace_root() with an explicit skip:\n%s\n' "$hits"
        return 1
    fi
    printf 'fixture-paths: no runtime CARGO_MANIFEST_DIR reads under %s (reviewed: %s)\n' \
        "${src_runtime_manifest_dir_roots[*]}" "${src_runtime_manifest_dir_reviewed[*]}"
}

check_fixture_paths() {
    release_test_child "$cargo_bin" nextest run -p cas --test builtin_archive_portability_test \
        builtin_inspection_tests_do_not_depend_on_the_checkout_at_runtime || return 1
    check_src_runtime_manifest_dir_reads
}

install_hub_web_dependencies() {
    local npm_bin="${NPM:-npm}"
    if [[ -e "$tmp_dir/hub-web-npm-installed" ]]; then
        return 0
    fi
    mkdir -p "$tmp_dir/npm-cache"
    (cd hub-web && \
        NPM_CONFIG_CACHE="$tmp_dir/npm-cache" \
        release_test_child "$npm_bin" ci --no-audit --no-fund) || return $?
    : >"$tmp_dir/hub-web-npm-installed"
}

check_hub_web_tests() {
    local npm_bin="${NPM:-npm}"
    if [[ ! -f hub-web/package.json ]]; then
        printf 'hub-web-tests: hub-web/package.json is not present; row not applicable to this release\n'
        return 0
    fi
    install_hub_web_dependencies || return $?
    (cd hub-web && \
        NPM_CONFIG_CACHE="$tmp_dir/npm-cache" release_test_child "$npm_bin" run typecheck && \
        NPM_CONFIG_CACHE="$tmp_dir/npm-cache" release_test_child "$npm_bin" test)
}

check_hub_web_dist_drift() {
    local npm_bin
    if [[ ! -f hub-web/package.json ]]; then
        printf 'hub-web-dist-drift: hub-web/package.json is not present; row not applicable to this release\n'
        return 0
    fi
    npm_bin="${NPM:-npm}"
    install_hub_web_dependencies || return $?
    (cd hub-web && \
        NPM_CONFIG_CACHE="$tmp_dir/npm-cache" \
        release_test_child "$npm_bin" run build && \
        git diff --exit-code -- dist)
}

check_hub_web_visual_qa() {
    local artifact_dir npm_bin
    if [[ -n "${RELEASE_GATE_HUB_WEB_VISUAL_QA:-}" ]]; then
        artifact_dir="$row_log_dir/hub-web-visual-qa"
        mkdir -p "$artifact_dir"
        release_test_child "$RELEASE_GATE_HUB_WEB_VISUAL_QA" "$artifact_dir"
        return $?
    fi
    # Releases before Commander carried no web package. Keep the row visible in
    # their receipt while making the new check fail closed whenever the package
    # is present but its runner is missing.
    if [[ ! -f hub-web/package.json ]]; then
        printf 'hub-web-visual-qa: hub-web/package.json is not present; row not applicable to this release\n'
        return 0
    fi
    [[ -f hub-web/scripts/visual-qa.mjs ]] || {
        printf 'hub-web-visual-qa: hub-web/scripts/visual-qa.mjs is missing\n'
        return 1
    }
    npm_bin="${NPM:-npm}"
    artifact_dir="$row_log_dir/hub-web-visual-qa"
    mkdir -p "$artifact_dir" "$tmp_dir/playwright"
    install_hub_web_dependencies && \
    (cd hub-web && \
        NPM_CONFIG_CACHE="$tmp_dir/npm-cache" \
        PLAYWRIGHT_BROWSERS_PATH="$tmp_dir/playwright" \
        release_test_child "$npm_bin" exec --yes --package=playwright -- playwright install chromium && \
        NPM_CONFIG_CACHE="$tmp_dir/npm-cache" \
        PLAYWRIGHT_BROWSERS_PATH="$tmp_dir/playwright" \
        release_test_child "$npm_bin" exec --yes --package=playwright -- node scripts/visual-qa.mjs \
            --artifact-dir "$artifact_dir")
}

check_workspace_tests() {
    release_test_child "$cargo_bin" check --workspace --tests
}

# A final release-profile artifact catches dependency backends that are absent
# from check/test profiles. No tags, uploads or remote mutation occur here.
release_binary_isa_toolchain() {
    local zigbuild_version zig_version objdump_bin objdump_version
    # Version belongs to the plugin's top-level CLI, not its zigbuild subcommand.
    zigbuild_version="$(cargo-zigbuild --version)" || return 1
    [[ -x "${ZIG:-}" ]] || return 1
    zig_version="$("$ZIG" version)" || return 1
    objdump_bin="$(release_portable_gnu_objdump)" || return 1
    objdump_version="$("$objdump_bin" --version)" || return 1
    [[ -n "$zigbuild_version" && -n "$zig_version" && -n "$objdump_version" ]] || return 1
    printf '%s\n' "$zigbuild_version" "$zig_version" "$objdump_version" | sha256sum | cut -d' ' -f1
}

check_release_binary_isa() {
    local target=x86_64-unknown-linux-gnu staging="$tmp_dir/release-binary-isa"
    local target_dir="${CARGO_TARGET_DIR:-target}"
    # --only does not dispatch the separate Zig discovery row. Resolve it here
    # too; zigbuild discovers the selected compiler through PATH, like release.sh.
    check_epic_worktree_zig || return $?
    # Use the same locked target/profile and C/C++ baseline as release.sh.
    # Keep the publisher's linker/rustflags: the native assembly compiler guard
    # installs its own linker and would change the cross-target artifact.
    release_test_child env PATH="$(dirname "$ZIG"):$PATH" \
        CFLAGS_x86_64_unknown_linux_gnu=-march=x86_64 \
        CXXFLAGS_x86_64_unknown_linux_gnu=-march=x86_64 \
        "$cargo_bin" zigbuild -p cas --release --target "$target" --locked || return $?
    mkdir -p "$staging" || return $?
    cp "$target_dir/$target/release/cas" "$staging/cas" || return $?
    # Audit the packaging copy rather than a dev/test executable. This also
    # retains the auditor's deterministic baseline and seeded-EVEX self-tests.
    release_test_child "$repo_root/scripts/test-check-portable-x86_64-isa.sh" "$staging/cas"
}

check_macos() {
    local rustup_bin="${RUSTUP:-rustup}" macos_cc="$tmp_dir/macos-check-cc"
    if ! command -v "$rustup_bin" >/dev/null 2>&1; then
        printf 'macos-check: rustup is unavailable; install rustup before checking aarch64-apple-darwin\n'
        return 1
    fi
    if ! "$rustup_bin" target add aarch64-apple-darwin; then
        printf 'macos-check: rustup target add aarch64-apple-darwin failed\n'
        return 1
    fi
    # Cross-target `cargo check` still runs C build scripts, but its output is
    # metadata-only and never links the target archive. Linux hosts reject the
    # Darwin-only flags and headers emitted by zstd-sys/ring/blake3, so compile
    # each C/assembly input as a tiny valid host object while preserving Cargo's
    # real Rust target analysis. This lane intentionally does not claim C ABI
    # or linker coverage; the macOS build lane owns that proof.
    cat >"$macos_cc" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
args=()
consume_next=false
compile=false
for arg in "$@"; do
    if [[ "$consume_next" == true ]]; then
        consume_next=false
        continue
    fi
    case "$arg" in
        -arch)
            consume_next=true
            continue
            ;;
        -m*-version-min=*|-fembed-bitcode|-fembed-bitcode-marker|-gfull)
            continue
            ;;
        -c)
            compile=true
            args+=("$arg")
            continue
            ;;
        *.c|*.cc|*.cpp|*.cxx|*.m|*.mm|*.S|*.s|-)
            if [[ "$compile" == true ]]; then
                continue
            fi
            args+=("$arg")
            continue
            ;;
    esac
    args+=("$arg")
done
if [[ "$compile" == true ]]; then
    printf '%s\n' 'int cas_release_gate_c_probe(void) { return 0; }' |
        cc "${args[@]}" -x c -
else
    exec cc "${args[@]}"
fi
EOF
    chmod +x "$macos_cc"
    release_test_child env RUSTC_WRAPPER= \
        "CC_aarch64-apple-darwin=$macos_cc" \
        "CC_aarch64_apple_darwin=$macos_cc" \
        TARGET_CC="$macos_cc" \
        "$cargo_bin" check --workspace --tests --target aarch64-apple-darwin
}

# The merge queue validates the whole workspace, so the suite and archive rows
# do too (cas-1f6e: a cas-mux snapshot test failed in the queue after a local
# `-p cas` gate passed). The non-cas crates add roughly a minute to each row.
run_assembly_compile() {
    local row="$1" started ended wall user system status=0
    shift
    local policy="${CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLICY:-}"
    [[ -n "$policy" ]] || policy='{}'
    local -a guarded=(python3 "$repo_root/scripts/assembly-memory.py" \
        --policy "$policy" \
        --events "$row_log_dir/compile-memory.jsonl" --root "$repo_root" -- "$@")
    started="$(date -u +%FT%TZ)"
    local LC_NUMERIC=C TIMEFORMAT='%R %U %S'
    if { time "${guarded[@]}" 2>&1; } 2>"$tmp_dir/$row-compile.time"; then
        :
    else
        status=$?
    fi
    ended="$(date -u +%FT%TZ)"
    read -r wall user system <"$tmp_dir/$row-compile.time"
    printf 'row\tstarted_utc\tended_utc\twall_s\tuser_s\tsystem_s\tstatus\tsource_sha\n' \
        >"$row_log_dir/compile-timing.tsv"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$row" "$started" "$ended" "$wall" "$user" "$system" "$status" "$cache_head" \
        >>"$row_log_dir/compile-timing.tsv"
    printf 'assembly compile: %s interval=%s to %s wall=%ss user=%ss system=%ss jobs=%s\n' \
        "$row" "$started" "$ended" "$wall" "$user" "$system" "${CARGO_BUILD_JOBS:-auto}"
    return "$status"
}

await_assembly_test_slot() {
    if [[ -z "${CAS_RELEASE_GATE_ASSEMBLY_SYNC_DIR:-}" ]]; then
        [[ -n "${CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLICY:-}" ]] || return 0
        local test_threads
        test_threads="$(python3 - "$repo_root/scripts/assembly-proof.py" \
            "$CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLICY" "$1" "$row_log_dir/memory-admission.json" <<'PY_ASSEMBLY_MEMORY'
import contextlib
import importlib.util
import json
from pathlib import Path
import sys

spec = importlib.util.spec_from_file_location("assembly_proof", sys.argv[1])
proof = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proof)
execution = {"phases": []}
try:
    with contextlib.redirect_stdout(sys.stderr):
        threads = proof.admit_phase(json.loads(sys.argv[2]), execution, sys.argv[3] + "-tests")
finally:
    Path(sys.argv[4]).write_text(json.dumps({"samples": execution["phases"]}) + "\n")
print(threads)
PY_ASSEMBLY_MEMORY
)" || return $?
        export NEXTEST_TEST_THREADS="$test_threads"
        return 0
    fi
    local test_threads
    test_threads="$(python3 - "$CAS_RELEASE_GATE_ASSEMBLY_SYNC_DIR" "$1" <<'PY_ASSEMBLY_SLOT'
import os
from pathlib import Path
import re
import sys
import time

sync, row = Path(sys.argv[1]), sys.argv[2]
(sync / ("compiled-" + row)).touch()
owner = int((sync / "owner").read_text())
deadline = time.monotonic() + 3600
while True:
    if (sync / "abort").exists():
        sys.exit("assembly test admission aborted: " + row)
    try:
        os.kill(owner, 0)
    except ProcessLookupError:
        sys.exit("assembly proof owner exited: " + row)
    if (sync / ("release-" + row)).exists():
        threads = (sync / ("release-" + row)).read_text().strip()
        if not re.fullmatch(r"[1-9][0-9]*", threads):
            sys.exit("invalid assembly test thread admission: " + row)
        print(threads)
        break
    if time.monotonic() >= deadline:
        sys.exit("assembly test admission timed out: " + row)
    time.sleep(0.1)
PY_ASSEMBLY_SLOT
)" || return $?
    export NEXTEST_TEST_THREADS="$test_threads"
}

check_nextest() {
    # The archive row executes the remaining workspace tests in the queue's
    # remapped environment. Cover its one exclusion here, once, instead of
    # running the entire suite a second time. --only nextest stays diagnostic.
    local -a selection=()
    if row_selected archive-mode; then
        selection=(--filterset 'binary_id(~component_output_test)')
    fi
    if [[ -n "${CAS_RELEASE_GATE_ASSEMBLY_SYNC_DIR:-}${CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLICY:-}" ]]; then
        run_assembly_compile nextest \
            bash "$gate_script_dir/release-test-env.sh" --home "$release_test_home" env \
            "$cargo_bin" nextest run --workspace "${selection[@]}" --no-run || return $?
        await_assembly_test_slot nextest || return $?
    fi
    release_test_child env \
        CAS_ROOT="${hermetic_cas_root:-}" CARGO="$cargo_bin" "$repo_root/scripts/run-verified-tests.sh" \
        nextest run --workspace "${selection[@]}" --no-fail-fast
}

check_doctests() {
    release_test_child env \
        CAS_ROOT="${hermetic_cas_root:-}" CARGO="$cargo_bin" "$repo_root/scripts/run-verified-tests.sh" test -p cas --doc
}

# A populated proxy.toml in an ancestor .cas is visible to any test that
# resolves project config by walking up from its cwd — release worktrees live at
# <repo>/.cas/worktrees/<name>, so the main checkout is always an ancestor.
#
# On 2026-09-03 that made three hermetic proxy tests fail during the v3.14.0
# gate. The suite is now immune (TestEnvGuard pins CAS_ROOT inside its temp
# HOME, cas-4ccc); this row covers whatever does not use that guard.
#
# The gate neutralizes rather than refuses: a release must not be blocked by the
# operator's own MCP configuration, and telling a human to move their files
# aside is how the original hour was lost. `CAS_ROOT` is the loader's documented
# override and wins ahead of both the worktree mapping and the ancestor walk, so
# pointing it at an empty directory makes the ancestor file unreachable for
# every child process. The file is named in the receipt either way, and never
# touched.
ancestor_proxy_config_files() {
    local probe files=()
    probe="$(cd "$repo_root" && pwd -P)"
    while [[ "$probe" != "/" && -n "$probe" ]]; do
        if [[ -s "$probe/.cas/proxy.toml" && "$probe/.cas" != "$repo_root/.cas" ]]; then
            files+=("$probe/.cas/proxy.toml")
        fi
        probe="$(dirname "$probe")"
    done
    (( ${#files[@]} )) && printf '%s\n' "${files[@]}"
    return 0
}

neutralize_ancestor_proxy_config() {
    local files=()
    while IFS= read -r line; do [[ -n "$line" ]] && files+=("$line"); done \
        < <(ancestor_proxy_config_files)
    (( ${#files[@]} )) || return 0

    hermetic_cas_root="$tmp_dir/hermetic-cas-root"
    mkdir -p "$hermetic_cas_root"
    export CAS_ROOT="$hermetic_cas_root"
    local file
    for file in "${files[@]}"; do
        printf 'note: ancestor .cas/proxy.toml visible to this worktree: %s\n' "$file"
    done
    printf 'note: running with CAS_ROOT=%s so ancestor-walking tests cannot read it\n' \
        "$hermetic_cas_root"
}

check_ancestor_proxy_config() {
    local files=()
    while IFS= read -r line; do [[ -n "$line" ]] && files+=("$line"); done \
        < <(ancestor_proxy_config_files)
    if (( ${#files[@]} == 0 )); then
        printf 'no ancestor .cas/proxy.toml above this worktree\n'
        return 0
    fi
    # Present, so the override must be in force and itself empty.
    if [[ -z "${CAS_ROOT:-}" || ! -d "${CAS_ROOT:-}" || -s "${CAS_ROOT:-}/proxy.toml" ]]; then
        printf 'ancestor .cas/proxy.toml is readable and CAS_ROOT is not pinned to an empty root: %s\n' \
            "${files[*]}"
        return 1
    fi
    printf 'neutralized %s ancestor proxy.toml file(s) with CAS_ROOT=%s: %s\n' \
        "${#files[@]}" "$CAS_ROOT" "${files[*]}"
    return 0
}

# Scratch bases must mirror the merge-queue runner: no ancestor directory may
# hold a .cas store, or every cas child that walks up from its cwd/TMPDIR
# finds the host's user-level store instead of the fixture.
assert_no_cas_ancestor() {
    local base="$1" probe
    probe="$(cd "$(dirname "$base")" 2>/dev/null && pwd -P || printf '%s' "$(dirname "$base")")"
    while [[ "$probe" != "/" && -n "$probe" ]]; do
        if [[ -d "$probe/.cas" ]]; then
            printf 'scratch base %s (from %s) has a .cas ancestor at %s; set CAS_RELEASE_GATE_HOME_DIR to a path with no .cas ancestor (the queue runner has none)\n' \
                "$base" "$scratch_base_origin" "$probe/.cas"
            return 1
        fi
        probe="$(dirname "$probe")"
    done
    [[ -d "/.cas" ]] && {
        printf 'scratch base %s (from %s) has a .cas ancestor at /.cas\n' "$base" "$scratch_base_origin"
        return 1
    }
    return 0
}

make_archive_path() {
    local name command
    for name in cargo rustc cargo-nextest git sh bash jq python3; do
        command="$(command -v "$name" || true)"
        [[ -n "$command" ]] || {
            printf 'archive-mode: required archive command is missing: %s\n' "$name" >&2
            return 1
        }
        ln -s "$command" "$archive_bin/$name"
    done
    if [[ "$(uname -s)" == Darwin ]]; then
        printf '/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin'
    else
        printf '/usr/bin:/bin'
    fi
}

check_archive_mode() {
    local archive_base archive_dir archive remap archive_tmp archive_cargo_home archive_path status
    archive_base="$scratch_base"
    mkdir -p "$(dirname "$archive_base")"
    assert_no_cas_ancestor "$archive_base" || return 1
    archive_dir="$(mktemp -d "${archive_base}.XXXXXX")"
    register_scratch "$archive_dir" || { rm -rf "$archive_dir"; return 1; }
    archive="$archive_dir/suite.tar.zst"
    remap="$archive_dir/workspace-remap"
    # The archive and extraction can be several GB: keep them on the checkout
    # disk. Test fixtures are disposable, and their SQLite/index commits must
    # use the same temp filesystem as native tests (cas-98a0).
    archive_tmp="$tmp_dir/archive-test-tmp"
    archive_cargo_home="$archive_dir/cargo-home"
    archive_bin="$archive_dir/bin"
    mkdir -p "$remap" "$archive_tmp" "$archive_cargo_home" "$archive_bin"
    assert_no_cas_ancestor "$archive_tmp" || return 1
    [[ -f Cargo.toml ]] || {
        printf 'archive-mode: root Cargo.toml is missing\n'
        return 1
    }
    # Mirror the merge-queue shard runner exactly: it HAS a checkout of the
    # tree, but at a different path than the build host, so compile-time
    # CARGO_MANIFEST_DIR reads fail while cwd-relative reads still work.
    # Empty package directories were stricter than CI and rejected a
    # pre-existing cwd-relative source inspection.
    # A detached git worktree, not `git archive`: the runner's checkout has a
    # .git, and tests that call git (check-ignore, rev-parse) need it.
    rmdir "$remap" 2>/dev/null || true
    git worktree add --detach "$remap" HEAD >/dev/null 2>&1 || {
        printf 'archive-mode: cannot create remap worktree at %s\n' "$remap"
        return 1
    }
    python3 "$repo_root/scripts/release_scratch.py" --repo "$repo_root" --path "$archive_dir" register-remap || return 1
    archive_path="$(make_archive_path)" || {
        status=$?
        return "$status"
    }
    local -a compile_command=(bash "$gate_script_dir/release-test-env.sh" --home "$release_test_home" env \
        "$cargo_bin" nextest archive --workspace --archive-file "$archive")
    if [[ -n "${CAS_RELEASE_GATE_ASSEMBLY_SYNC_DIR:-}${CAS_RELEASE_GATE_ASSEMBLY_MEMORY_POLICY:-}" ]]; then
        compile_command=(run_assembly_compile archive-mode "${compile_command[@]}")
    fi
    if "${compile_command[@]}"; then
        :
    else
        status=$?
        return "$status"
    fi
    [[ -s "$archive" ]] || {
        printf 'archive-mode: nextest did not create %s\n' "$archive"
        return 1
    }
    local archive_bytes
    archive_bytes="$(wc -c <"$archive" | tr -d '[:space:]')"
    printf '%s\n' "$archive_bytes" >"$scratch_archive_history_file.tmp.$$"
    mv "$scratch_archive_history_file.tmp.$$" "$scratch_archive_history_file"
    if [[ -n "$archive_size_file" ]]; then
        mkdir -p "$(dirname "$archive_size_file")"
        printf '%s\n' "$archive_bytes" >"$archive_size_file.tmp.$$"
        mv "$archive_size_file.tmp.$$" "$archive_size_file"
    fi
    printf 'archive-mode: archive size %s bytes recorded%s\n' "$archive_bytes" \
        "${archive_size_file:+ in $archive_size_file}"
    if await_assembly_test_slot archive-mode; then
        :
    else
        status=$?
        return "$status"
    fi
    # nextest canonicalizes --extract-to before extracting, so it must exist.
    # Match the private 0700 mktemp base, with the same process owner/group.
    # The guardian removes the whole archive_dir after all descendants exit.
    mkdir -p -m 700 "$archive_dir/extract" || {
        status=$?
        return "$status"
    }
    printf 'archive-mode: test TMPDIR=%s; extraction=%s; workspace-remap=%s\n' \
        "$archive_tmp" "$archive_dir/extract" "$remap"
    # --extract-to decouples the large disk extraction from test-time TMPDIR.
    # Producer and remap remain outside disposable roots in assembly proofs;
    # missing-wrapper, empty CARGO_HOME, PATH and snapshot exclusion still
    # exercise the queue consumer's independent archive environment.
    if (
        cd "$archive_dir"
        release_test_child env -u COLUMNS \
            TMPDIR="$archive_tmp" \
            CARGO_HOME="$archive_cargo_home" RUSTC_WRAPPER=/nonexistent/sccache \
            INSTA_WORKSPACE_ROOT="$remap" CARGO="$cargo_bin" \
            PATH="$archive_bin${archive_path:+:$archive_path}" \
            "$remap/scripts/run-verified-tests.sh" nextest run --archive-file "$archive" \
            --extract-to "$archive_dir/extract" --workspace-remap "$remap" --no-fail-fast \
            --filterset 'not binary_id(~component_output_test)'
    ); then
        status=0
    else
        status=$?
    fi
    return "$status"
}

check_snapshot_portability() {
    # The deep TMPDIR must live OUTSIDE every checkout: worktrees sit under the
    # main repo's .cas/, so a temp dir inside the tree lets find_cas_root walk
    # up into the real project store and the snapshot captures live data.
    local deep_base="$scratch_base"
    mkdir -p "$(dirname "$deep_base")"
    assert_no_cas_ancestor "$deep_base" || return 1
    # The scratch root is kept in its own variable so it can be removed: this
    # row used to leak one <base>.snap.XXXXXX directory per invocation, where
    # check_archive_mode has always cleaned up after itself. That was survivable
    # while the base was an opt-in path; now that /var/tmp/cas-release-gate is
    # the default on every host, an uncleaned run would accumulate there for
    # everyone, including once per self-test fixture.
    local deep_root deep_tmp
    deep_root="$(mktemp -d "${deep_base}.snap.XXXXXX")"
    register_scratch "$deep_root" || { rm -rf "$deep_root"; return 1; }
    deep_tmp="$deep_root/$(printf 'deep-temp-path-%.0s' {1..12})"
    mkdir -p "$deep_tmp"
    # COLUMNS must be absent, rather than merely empty: terminal-width probes
    # commonly distinguish the two states.
    local status
    release_test_child env -u COLUMNS INSTA_UPDATE=no TMPDIR="$deep_tmp" \
        "$cargo_bin" nextest run -p cas --test component_output_test
    status=$?
    # Never leave insta's pending-snapshot artifacts behind: they would fail
    # the working-tree row of this same gate.
    find . -path ./target -prune -o -name '*.snap.new' -print0 2>/dev/null | xargs -0 rm -f --
    return "$status"
}

check_builtin_projections() {
    release_test_child "$cargo_bin" nextest run -p cas --test builtin_flavor_drift_test \
        root_managed_projections_stay_synced_and_project_skills_stay_ignored || return $?
    [[ -x "$reference_history_script" ]] || {
        printf 'builtin-projections: missing executable %s\n' "$reference_history_script"
        return 1
    }
    "$reference_history_script" || return $?
    if ! git diff --quiet -- cas-cli/src/builtins/reference-history.json; then
        printf 'reference-history: regenerated ledger differs from the committed file\n'
        git diff -- cas-cli/src/builtins/reference-history.json
        return 1
    fi
}

check_changelog_and_versions() {
    local file current
    local -a release_crates=(
        cas-cli/Cargo.toml
        crates/cas-types/Cargo.toml
        crates/cas-search/Cargo.toml
        crates/cas-store/Cargo.toml
        crates/cas-core/Cargo.toml
        crates/cas-mcp/Cargo.toml
    )

    grep -Eq '^## \[Unreleased\]$' CHANGELOG.md || {
        printf 'changelog: CHANGELOG.md is missing ## [Unreleased]\n'
        return 1
    }
    grep -Eq "^## \[$version\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$" CHANGELOG.md || {
        printf 'changelog: missing release heading for %s\n' "$version"
        return 1
    }
    if ! awk -v version="$version" '
        $0 ~ "^## \\[" version "\\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$" { in_section=1; next }
        in_section && /^## \[/ { in_section=0 }
        in_section && /^- / { found=1 }
        END { exit(found ? 0 : 1) }
    ' CHANGELOG.md; then
        printf 'changelog: section [%s] has no bullet\n' "$version"
        return 1
    fi

    for file in "${release_crates[@]}"; do
        [[ -f "$file" ]] || {
            printf 'version-alignment: missing %s\n' "$file"
            return 1
        }
        current="$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$file" | head -n1)"
        if [[ "$current" != "$version" ]]; then
            printf 'version-alignment: %s is %s; expected %s\n' "$file" "${current:-missing}" "$version"
            return 1
        fi
    done
}

check_publish_toolchain() {
    python3 "$repo_root/scripts/check-release-publish-toolchain.py" "$repo_root"
}

check_release_script() {
    [[ -f scripts/release.sh ]] || {
        printf 'release-script: scripts/release.sh is missing\n'
        return 1
    }
    grep -qF 'target/$target/release/build"/blake3-*' scripts/release.sh
    grep -qF 'target/$target/release/.fingerprint"/blake3-*' scripts/release.sh
    grep -qF 'Pre-warming rule: in a tag worktree' scripts/release.sh
    grep -qF 'audit-only and remote-safe' scripts/release.sh
}

check_release_notes_shell_injection() {
    python3 scripts/check-workflow-run-interpolation.py
}

check_procedure_guardrails() {
    local skill="cas-cli/src/builtins/skills/cas-cut-release/SKILL.md"
    [[ -f "$skill" ]] || {
        printf 'procedure-guardrails: missing %s\n' "$skill"
        return 1
    }
    grep -qF 'kill -0' "$skill"
    grep -qF 'full suite on the assembled tree' "$skill"
    grep -qF 'stranded_branch_override' "$skill"
    grep -qF 'release-published-receipt.sh --write-draft' "$skill"
    grep -qF 'cas --version' "$skill"
    grep -qF 'Scoped Validation' "$skill"
    grep -qF 'ledger is the last prep step' "$skill"
    grep -qF 'cause class' "$skill"
    grep -qF '9.99.x' "$skill"
    grep -qF 'workers never poll CI' "$skill"
    grep -qF 'reviewed snapshot update' "$skill"
    grep -qF 'competing release' "$skill"
    grep -qF 'merge-queue GraphQL query' "$skill"
    grep -qF 'CAS_RELEASE_ENV_FILE' "$skill"
    grep -qF 'annotated tag peels' "$skill"
    grep -qF 'four Slack POSTED' "$skill"
    grep -qF 'refresh_binary_version' "$skill"
    grep -qF 'release.tag-complete.epoch' "$skill"
    grep -qF 'release-published.receipt' "$skill"
}

check_test_targets() {
    python3 scripts/cas-test-targets.py cas-cli --check
}

check_markdown_lint() {
    python3 scripts/check-changed-markdown.py "${fast_base:-HEAD^}"
}

check_test_env() {
    [[ -f scripts/check-test-env.py ]] || {
        printf 'test-env: scripts/check-test-env.py is missing\n'
        return 1
    }
    if [[ "$fast_rows" == true ]]; then
        python3 scripts/check-test-env.py --changed-since "${fast_base:-HEAD^}" --changed-paths || return $?
        # Fixture suites belong to the full gate, unless this lane changes the
        # lint implementation/fixtures themselves.
        if git diff --quiet "${fast_base:-HEAD^}" -- scripts/check-test-env.py scripts/rust_test_source.py \
            scripts/test-check-test-env.py scripts/fixtures/test-env-lint.json; then
            return 0
        fi
    else
        python3 scripts/check-test-env.py || return $?
    fi
    if [[ -f scripts/test-check-test-env.py ]]; then
        release_test_child python3 scripts/test-check-test-env.py
    fi
}

check_test_shape() {
    [[ -f scripts/check-test-shape.py ]] || {
        printf 'test-shape: scripts/check-test-shape.py is missing\n'
        return 1
    }
    if [[ "$fast_rows" == true ]]; then
        python3 scripts/check-test-shape.py --changed-since "${fast_base:-HEAD^}" || return $?
    else
        python3 scripts/check-test-shape.py || return $?
    fi
    if [[ -f scripts/test-check-test-shape.py ]]; then
        release_test_child python3 scripts/test-check-test-shape.py
    fi
}

check_builtin_doc_hygiene() {
    python3 scripts/check-builtin-doc-hygiene.py || return $?
    python3 scripts/check-builtin-contract-phrases.py
}

# Fast mode runs a lane row only when the lane changes one of its inputs; the
# full gate always runs it.
lane_touches() {
    [[ "$fast_rows" == true ]] || return 0
    ! git diff --quiet "${fast_base:-HEAD^}" -- "$@"
}

check_journey_catalog() {
    if ! lane_touches docs/qa/journeys.md hub-web/e2e scripts/journeys-for-diff.py; then
        printf 'journey-catalog: no journey catalog or spec change\n'
        return 0
    fi
    python3 scripts/journeys-for-diff.py --check
}

check_builtin_skill_limits() {
    if ! lane_touches cas-cli/src/builtins.rs cas-cli/src/builtins AGENTS.md CLAUDE.md \
        scripts/check-builtin-skill-limits.py; then
        printf 'builtin-skill-limits: no builtin skill change\n'
        return 0
    fi
    python3 scripts/check-builtin-skill-limits.py .
}

check_doctor_snapshot() {
    if ! lane_touches cas-cli/src/cli/doctor.rs cas-cli/tests/snapshots scripts/check-doctor-snapshot.py; then
        printf 'doctor-snapshot: no doctor or snapshot change\n'
        return 0
    fi
    if [[ "$fast_rows" == true ]]; then
        python3 scripts/check-doctor-snapshot.py . --base "${fast_base:-HEAD^}"
    else
        python3 scripts/check-doctor-snapshot.py .
    fi
}

check_migration_registry() {
    if ! lane_touches cas-cli/src/migration scripts/check-migration-registry.py; then
        printf 'migration-registry: no migration change\n'
        return 0
    fi
    python3 scripts/check-migration-registry.py .
}

# The no-build half of fixture-paths (cas-cc4d: it failed at assembly). Fast mode
# scans the lane's changed cas-cli/src Rust files; the full gate scans all.
check_fixture_paths_src() {
    if [[ "$fast_rows" != true ]]; then
        check_src_runtime_manifest_dir_reads
        return
    fi
    local -a changed=()
    local path hits=''
    while IFS= read -r path; do
        [[ "$path" == *.rs && -f "$path" ]] && changed+=("$path")
    done < <(git diff --name-only --diff-filter=ACMR "${fast_base:-HEAD^}" -- \
        cas-cli/src ':(glob)crates/*/src/**' ':(glob)crates/*/tests/**')
    if [[ "${#changed[@]}" -eq 0 ]]; then
        printf 'fixture-paths-src: no cas-cli/src or crate src/tests Rust change\n'
        return 0
    fi
    hits="$(src_runtime_manifest_dir_hits "${changed[@]}")"
    if [[ -n "$hits" ]]; then
        printf 'fixture-paths-src: cas-cli/src test modules read the producer checkout at runtime; use include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/..")) or cas::test_paths::workspace_root() with an explicit skip:\n%s\n' "$hits"
        return 1
    fi
    printf 'fixture-paths-src: no runtime CARGO_MANIFEST_DIR reads in %s changed file(s)\n' "${#changed[@]}"
}

check_ci_script_tests_changed() (
    if [[ "$fast_rows" != true ]]; then
        printf 'ci-script-tests-changed: the full gate runs every entry in ci-script-tests\n'
        exit 0
    fi
    release_test_child python3 scripts/ci-script-tests-for-diff.py --base "${fast_base:-HEAD^}"
)

check_ci_script_tests() (
    # This is the queue's script-only preflight, not a Cargo test target.
    # Nested gate self-tests own their receipts and synchronization. Otherwise
    # they truncate this row's timing.tsv and append synthetic failed rows.
    # Keep the outer gate's controls intact by scrubbing only this subshell.
    release_test_child \
        make -C cas-cli test-ci-tiers
)

check_working_tree() {
    local untracked
    if ! git diff --quiet; then
        git diff --stat
        printf 'working-tree: unstaged changes are present\n'
        return 1
    fi
    if ! git diff --cached --quiet; then
        git diff --cached --stat
        printf 'working-tree: staged changes are present\n'
        return 1
    fi
    untracked="$(git ls-files --others --exclude-standard)"
    if [[ -n "$untracked" ]]; then
        printf '%s\n' "$untracked"
        printf 'working-tree: untracked files are present\n'
        return 1
    fi
}

printf '=== CAS RELEASE GATE RECEIPT ===\n'
printf 'version: %s\n' "$version"
printf 'repository: %s\n' "$repo_root"
printf 'scratch base: %s (from %s)\n' "$scratch_base" "$scratch_base_origin"
printf 'archive size receipts: prior=%s per-run=%s\n' "$scratch_archive_history_file" \
    "${archive_size_file:-not-configured}"
printf 'init watchdog budget: %ss (from %s; cas init clamps at 3600s)\n' \
    "$CAS_INIT_TIMEOUT_SECS" "$init_timeout_origin"
if [[ "$fast_rows" == false ]]; then
    neutralize_ancestor_proxy_config
fi

run_check scratch-base \
    'parent writable; same mount as checkout; no .cas ancestor; free bytes >= 2x last archive' \
    check_scratch_base
if row_selected scratch-base && [[ "${#failures[@]}" -gt 0 ]]; then
    printf 'RELEASE GATE FAILED: scratch-base (aborted before costly or mutating rows)\n'
    exit 1
fi
run_check epic-worktree-fresh \
    'epic worktree is clean and HEAD matches its claimed epic ref' \
    check_epic_worktree_fresh
run_check epic-worktree-zig \
    'resolve and export an executable Zig from env, epic worktree, or main checkout' \
    check_epic_worktree_zig
run_check publish-toolchain \
    'python3 scripts/check-release-publish-toolchain.py (real zigbuild parser, no build)' \
    check_publish_toolchain
run_check failure-log \
    "parse $failure_log_rel; every entry maps to a gate check id or manual:" \
    check_failure_log
run_check assemble-stale-base \
    'python3 scripts/test-release-integration.py (stale origin/main recovery and identity-safe base-only heal)' \
    check_assemble_stale_base
run_check ancestor-proxy-config \
    'no populated .cas/proxy.toml above this worktree that ancestor-walking tests could read' \
    check_ancestor_proxy_config
run_check version-literals \
    'find source/test files for <version> (excluding manifests, CHANGELOG, reference-history, failure-log)' \
    check_version_literals
run_check ci-script-tests \
    'make -C cas-cli test-ci-tiers (factory identity scrubbed)' \
    check_ci_script_tests
if row_selected ci-script-tests && [[ "${failures[*]}" == *ci-script-tests* ]]; then
    printf 'RELEASE GATE FAILED: %s (aborted before build and Rust suite rows)\n' "${failures[*]}"
    exit 1
fi
run_check hub-web-tests \
    'npm ci --no-audit --no-fund && npm run typecheck && npm test' \
    check_hub_web_tests
if row_selected hub-web-tests && [[ "${failures[*]}" == *hub-web-tests* ]]; then
    printf 'RELEASE GATE FAILED: %s (aborted before build and Rust suite rows)\n' "${failures[*]}"
    exit 1
fi
run_check release-binary-isa \
    "$cargo_bin zigbuild -p cas --release --target x86_64-unknown-linux-gnu --locked (baseline C/C++ flags); scripts/test-check-portable-x86_64-isa.sh <packaged-cas>" \
    check_release_binary_isa
if row_selected release-binary-isa && [[ "${failures[*]}" == *release-binary-isa* ]]; then
    printf 'RELEASE GATE FAILED: %s (release binary ISA audit refused before pr-body/pipeline)\n' "${failures[*]}"
    exit 1
fi
run_check fixture-paths \
    "$cargo_bin nextest run -p cas --test builtin_archive_portability_test builtin_inspection_tests_do_not_depend_on_the_checkout_at_runtime; no runtime CARGO_MANIFEST_DIR reads under cas-cli/src, crates/*/src or crates/*/tests" \
    check_fixture_paths
run_check workspace-tests \
    "$cargo_bin check --workspace --tests" \
    check_workspace_tests
run_check macos-check \
    "$cargo_bin check --workspace --tests --target aarch64-apple-darwin (rustup target add preflight)" \
    check_macos
run_check hub-web-dist-drift \
    'npm ci --no-audit --no-fund && npm run build && git diff --exit-code -- dist' \
    check_hub_web_dist_drift
run_check hub-web-visual-qa \
    'npm exec --yes --package=playwright -- node scripts/visual-qa.mjs --artifact-dir <gate-scratch>/hub-web-visual-qa' \
    check_hub_web_visual_qa
run_check nextest \
    "$cargo_bin nextest run --workspace (factory environment scrubbed; archive-selected: snapshot complement)" \
    check_nextest
run_check doctests \
    "$cargo_bin test -p cas --doc" \
    check_doctests
run_check archive-mode \
    "env -u CAS_FACTORY_SESSION -u CAS_AGENT_ROLE -u CAS_AGENT_NAME -u CAS_SUPERVISOR_NAME -u CAS_AGENT_ID $cargo_bin nextest archive --workspace --archive-file <home-disk>/suite.tar.zst; archive run outside checkout with remap and rg removed" \
    check_archive_mode
run_check snapshot-portability \
    'env -u COLUMNS TMPDIR=<deep path> cargo nextest run -p cas --test component_output_test' \
    check_snapshot_portability
run_check builtin-projections \
    "$cargo_bin nextest run -p cas --test builtin_flavor_drift_test root_managed_projections...; regenerate ledger; git diff --quiet" \
    check_builtin_projections
run_check changelog-and-versions \
    'CHANGELOG [Unreleased]/[version] bullet plus six release-train Cargo.toml versions' \
    check_changelog_and_versions
run_check release-script \
    'release.sh stale duplicate cleanup and audit-only pre-warm contract' \
    check_release_script
run_check release-notes-shell-injection \
    'all workflow run blocks reject GitHub expression interpolation' \
    check_release_notes_shell_injection
run_check procedure-guardrails \
    'cas-cut-release reconciliation, queue, PID, receipt, and host guardrails' \
    check_procedure_guardrails
run_check test-targets 'python3 scripts/cas-test-targets.py cas-cli --check' check_test_targets
run_check markdown-lint 'markdownlint-cli2 on changed Markdown (repository policy)' check_markdown_lint
if [[ "$fast_rows" == true && ! -f scripts/check-test-shape.py ]]; then
    # Either sibling lane may merge first. A missing checker is visible rather
    # than manufactured green evidence; the full gate always requires it.
    print_result SKIP test-shape 'checker not installed yet (sibling lint lane)'
else
    run_check test-shape 'python3 scripts/check-test-shape.py (changed lane in fast mode)' check_test_shape
fi
run_check test-env 'process-state test lint (affected crate paths in fast mode) and strict baseline ratchet' check_test_env
run_check builtin-doc-hygiene 'shared operator-data policy on builtin sources' check_builtin_doc_hygiene
run_check journey-catalog 'python3 scripts/journeys-for-diff.py --check (catalog steps match test.step titles)' \
    check_journey_catalog
run_check builtin-skill-limits 'python3 scripts/check-builtin-skill-limits.py (description, size and line limits)' \
    check_builtin_skill_limits
run_check doctor-snapshot 'python3 scripts/check-doctor-snapshot.py (row groups; new doctor phases update the snapshot)' \
    check_doctor_snapshot
run_check migration-registry 'python3 scripts/check-migration-registry.py (every migration declared, registered, in order)' \
    check_migration_registry
run_check ci-script-tests-changed 'python3 scripts/ci-script-tests-for-diff.py (script tests for changed scripts)' \
    check_ci_script_tests_changed
run_check fixture-paths-src 'no runtime CARGO_MANIFEST_DIR reads in cas-cli/src, crates/*/src, crates/*/tests (changed files in fast mode)' \
    check_fixture_paths_src
run_check working-tree \
    'git diff --quiet; git diff --cached --quiet; git ls-files --others --exclude-standard' \
    check_working_tree

if [[ "${#failures[@]}" -gt 0 ]]; then
    printf 'RELEASE GATE FAILED: %s\n' "${failures[*]}"
    exit 1
fi

if [[ -n "$only_rows" ]]; then
    printf 'RELEASE GATE PASSED: selected checks are green for %s: %s\n' "$version" "$selected_rows_summary"
else
    printf 'RELEASE GATE PASSED: all checks are green for %s\n' "$version"
fi

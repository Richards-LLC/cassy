#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cutover="$repo_root/scripts/cutover-cassy-actions-cache.sh"
suite_root="$(mktemp -d "${TMPDIR:-/tmp}/cas-a352-cutover-test.XXXXXX")"
trap 'chmod -R u+rwX "$suite_root" 2>/dev/null || true; rm -rf "$suite_root"' EXIT
passes=0

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

assert_file_text() {
    local path="$1" expected="$2" label="$3" actual
    [[ -f "$path" ]] || fail "$label: missing $path"
    actual="$(<"$path")"
    [[ "$actual" == "$expected" ]] || fail "$label: expected '$expected', got '$actual'"
}

assert_absent() {
    [[ ! -e "$1" ]] || fail "$2: unexpected $1"
}

write_mock() {
    local root="$1"
    cat >"$root/bin/cassy-cutover-mock" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
name="$(basename "$0")"
state="$MOCK_ROOT/state"

case "$name" in
    gh)
        printf '%q ' "$@" >>"$state/gh.log"
        printf '\n' >>"$state/gh.log"
        if [[ "${1:-}" == api ]]; then
            endpoint="${2:-}"
            if [[ "$endpoint" == user ]]; then
                exit 0
            fi
            if [[ "$endpoint" == orgs/Richards-LLC/actions/runners ]]; then
                jq_expr=
                while (($#)); do
                    [[ "$1" == --jq ]] && jq_expr="${2:-}"
                    shift
                done
                busy="${MOCK_RUNNER_BUSY:-0}"
                if [[ "$jq_expr" == *'.busy == true'* ]]; then
                    printf '%s\n' "$busy"
                elif [[ "$jq_expr" == *'| length'* ]]; then
                    if [[ "$busy" == 0 ]]; then
                        printf '%s\n' "${MOCK_ONLINE_COUNT:-2}"
                    else
                        printf '0\n'
                    fi
                else
                    printf '{\n  "name": "soundwave-cas-ci",\n  "status": "online",\n  "busy": %s\n}\n' "$([[ "$busy" == 0 ]] && printf false || printf true)"
                    printf '{\n  "name": "soundwave-cas-ci-2",\n  "status": "online",\n  "busy": false\n}\n'
                fi
                exit 0
            fi
            if [[ "$endpoint" == repos/Richards-LLC/cassy/actions/variables/* ]]; then
                variable="${endpoint##*/}"
                if [[ "${MOCK_GH_GET_ERROR_NAME:-}" == "$variable" ]]; then
                    printf 'gh: service unavailable (HTTP 503)\n' >&2
                    exit 1
                fi
                if [[ -f "$state/routes/$variable" ]]; then
                    cat "$state/routes/$variable"
                else
                    printf 'gh: Not Found (HTTP 404)\n' >&2
                    exit 1
                fi
                exit 0
            fi
            printf 'unexpected gh api endpoint: %s\n' "$endpoint" >&2
            exit 2
        fi
        if [[ "${1:-}" == variable && "${2:-}" == get ]]; then
            variable="${3:?}"
            if [[ "${MOCK_GH_GET_ERROR_NAME:-}" == "$variable" ]]; then
                printf 'gh: service unavailable (HTTP 503)\n' >&2
                exit 1
            fi
            [[ -f "$state/routes/$variable" ]] || exit 1
            cat "$state/routes/$variable"
            exit 0
        fi
        if [[ "${1:-}" == variable && "${2:-}" == set ]]; then
            variable="${3:?}"
            set_count=$(( $(<"$state/gh-set-count") + 1 ))
            printf '%s\n' "$set_count" >"$state/gh-set-count"
            if [[ "${MOCK_GH_SET_FAIL_AT:-0}" == "$set_count" ]]; then
                printf 'gh: injected variable-set failure (HTTP 503)\n' >&2
                exit 1
            fi
            shift 3
            value=
            while (($#)); do
                [[ "$1" == --body ]] && value="${2:-}"
                shift
            done
            printf '%s\n' "$value" >"$state/routes/$variable"
            exit 0
        fi
        if [[ "${1:-}" == variable && "${2:-}" == delete ]]; then
            variable="${3:?}"
            if [[ "${MOCK_GH_DELETE_ERROR_NAME:-}" == "$variable" ]]; then
                printf 'gh: service unavailable (HTTP 503)\n' >&2
                exit 1
            fi
            rm -f "$state/routes/$variable"
            exit 0
        fi
        printf 'unexpected gh command\n' >&2
        exit 2
        ;;
    sudo)
        [[ "${1:-}" == -n && "${2:-}" == true ]] && exit 0
        if [[ "${1:-}" == -u ]]; then
            shift 2
        fi
        if [[ "${1:-}" == install ]]; then
            shift
            args=()
            while (($#)); do
                case "$1" in
                    -o|-g) shift 2 ;;
                    *) args+=("$1"); shift ;;
                esac
            done
            exec /usr/bin/install "${args[@]}"
        fi
        if [[ "${1:-}" == touch && "${2:-}" == "$MOCK_ROOT/var/lib/cassy-actions/cache/"* && "$(<"$state/mounted")" == 1 ]]; then
            relative="${2#"$MOCK_ROOT/var/lib/cassy-actions/cache/"}"
            exec /usr/bin/touch "$MOCK_ROOT/mnt/shockwave/home/pippenz/cassy-actions/cache/$relative"
        fi
        [[ "${1:-}" == mv ]] && printf 'mv %s\n' "$*" >>"$state/host-mutation.log"
        exec "$@"
        ;;
    systemctl)
        command="${1:-}"
        shift || true
        printf '%s %s\n' "$command" "$*" >>"$state/systemctl.log"
        case "$command" in
            is-active)
                [[ "${1:-}" == --quiet ]] && shift
                [[ "$(<"$state/services/${1:?}")" == active ]]
                ;;
            stop)
                stop_count=$(( $(<"$state/stop-count") + 1 ))
                printf '%s\n' "$stop_count" >"$state/stop-count"
                if [[ "${MOCK_SYSTEMCTL_STOP_FAIL_AT:-0}" == "$stop_count" ]]; then
                    printf 'inactive\n' >"$state/services/${1:?}"
                    printf 'injected systemctl stop failure\n' >&2
                    exit 1
                fi
                for service in "$@"; do printf 'inactive\n' >"$state/services/$service"; done
                ;;
            start)
                for service in "$@"; do printf 'active\n' >"$state/services/$service"; done
                ;;
            daemon-reload) ;;
            show)
                printf 'Id=mock\nActiveState=active\nSubState=running\nMainPID=123\nResult=success\n'
                ;;
            *) printf 'unexpected systemctl command: %s\n' "$command" >&2; exit 2 ;;
        esac
        ;;
    systemd-analyze) exit 0 ;;
    mountpoint)
        [[ "${1:-}" == -q ]] && shift
        if [[ "$1" == "$MOCK_ROOT/mnt/shockwave" ]]; then
            exit 0
        fi
        if [[ "$1" == "$MOCK_ROOT/var/lib/cassy-actions/cache" ]]; then
            [[ "$(<"$state/mounted")" == 1 ]]
            exit
        fi
        exit 1
        ;;
    mount)
        [[ "${1:-}" == "$MOCK_ROOT/var/lib/cassy-actions/cache" ]] || exit 2
        printf '1\n' >"$state/mounted"
        ;;
    umount)
        [[ "$1" == "$MOCK_ROOT/var/lib/cassy-actions/cache" ]] || exit 2
        printf 'umount %s\n' "$1" >>"$state/host-mutation.log"
        printf '0\n' >"$state/mounted"
        ;;
    findmnt)
        output=
        target=
        selector=
        while (($#)); do
            case "$1" in
                -o) output="${2:-}"; shift 2 ;;
                -T|--target) selector=target; target="${2:-}"; shift 2 ;;
                -M|--mountpoint) selector=mountpoint; target="${2:-}"; shift 2 ;;
                -n) shift ;;
                *) shift ;;
            esac
        done
        case "$output" in
            MAJ:MIN)
                if [[ "$target" == "$MOCK_ROOT/mnt/shockwave/home/pippenz/cassy-actions/cache" ]]; then
                    printf '%s\n' "${MOCK_DEST_DEVICE:-259:1}"
                else
                    printf '259:1\n'
                fi
                ;;
            TARGET)
                if [[ "$selector" == target && "$target" == "$MOCK_ROOT/mnt/shockwave/home/pippenz/cassy-actions/cache" ]]; then
                    printf '%s\n' "$MOCK_ROOT/mnt/shockwave"
                else
                    printf '%s\n' "$target"
                fi
                ;;
            FSROOT)
                if [[ "$target" == "$MOCK_ROOT/var/lib/cassy-actions/cache" ]]; then
                    printf '%s\n' "${MOCK_SOURCE_FSROOT:-/home/pippenz/cassy-actions/cache}"
                else
                    printf '/\n'
                fi
                ;;
            *) printf 'TARGET SOURCE FSTYPE OPTIONS SIZE USED AVAIL\n%s /dev/mock ext4 rw 1T 1G 999G\n' "$target" ;;
        esac
        ;;
    pgrep)
        [[ "${MOCK_RUNNER_BUSY:-0}" == 1 ]]
        ;;
    rsync)
        printf '%s\n' "$*" >>"$state/rsync.log"
        rsync_count=$(( $(<"$state/rsync-count") + 1 ))
        printf '%s\n' "$rsync_count" >"$state/rsync-count"
        if [[ "${MOCK_RSYNC_FAIL_AT:-0}" == "$rsync_count" ]]; then
            printf 'injected rsync failure\n' >&2
            exit 23
        fi
        dry_run=0
        for arg in "$@"; do [[ "$arg" == *n* && "$arg" == -* ]] && dry_run=1; done
        source="${@: -2:1}"
        destination="${@: -1}"
        if [[ "$dry_run" == 1 ]]; then
            if [[ "${MOCK_RSYNC_DRY_DIFF:-0}" == 1 ]]; then
                printf 'mock/changed-file\n'
            else
                diff -qr "${source%/}" "${destination%/}" || true
            fi
        else
            /bin/cp -a "${source%/}/." "${destination%/}/"
            printf 'mock rsync copied fixture\n'
        fi
        ;;
    sha256sum)
        path="${@: -1}"
        if [[ "${MOCK_HASH_MISMATCH:-0}" == 1 && "$path" == "$MOCK_ROOT/mnt/shockwave/"* ]]; then
            printf 'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff  %s\n' "$path"
        else
            exec /usr/bin/sha256sum "$@"
        fi
        ;;
    sleep) exit 0 ;;
    *) printf 'unexpected mock executable: %s\n' "$name" >&2; exit 2 ;;
esac
MOCK
    chmod +x "$root/bin/cassy-cutover-mock"
    local command
    for command in gh sudo systemctl systemd-analyze mountpoint mount umount findmnt pgrep rsync sha256sum sleep; do
        ln -s cassy-cutover-mock "$root/bin/$command"
    done
}

setup_fixture() {
    local name="$1" root
    root="$suite_root/$name"
    mkdir -p "$root/bin" "$root/state/routes" "$root/state/services" \
        "$root/var/lib/cassy-actions/cache/cargo-target" \
        "$root/mnt/shockwave/home/pippenz/cassy-actions/cache" \
        "$root/etc/systemd/system" "$root/artifacts"
    : >"$root/.cassy-actions-cutover-test-root"
    printf '0\n' >"$root/state/mounted"
    printf '0\n' >"$root/state/gh-set-count"
    printf '0\n' >"$root/state/stop-count"
    printf '0\n' >"$root/state/rsync-count"
    : >"$root/state/systemctl.log"
    : >"$root/state/gh.log"
    : >"$root/state/rsync.log"
    : >"$root/state/host-mutation.log"
    printf 'enabled\n' >"$root/state/routes/CASSY_MERGE_QUEUE_SELF_HOSTED"
    printf 'enabled\n' >"$root/state/routes/CASSY_RELEASE_SELF_HOSTED"
    printf 'active\n' >"$root/state/services/cassy-actions-runner.service"
    printf 'active\n' >"$root/state/services/cassy-actions-runner-2.service"
    printf 'rootfs / ext4 defaults 0 1\n' >"$root/etc/fstab"
    printf 'old unit 1\n' >"$root/etc/systemd/system/cassy-actions-runner.service"
    printf 'old unit 2\n' >"$root/etc/systemd/system/cassy-actions-runner-2.service"
    printf 'alpha\n' >"$root/var/lib/cassy-actions/cache/cargo-target/one.bin"
    printf 'beta\n' >"$root/var/lib/cassy-actions/cache/cargo-target/two.bin"
    printf 'gamma\n' >"$root/var/lib/cassy-actions/cache/cargo-target/three.bin"
    /bin/cp -a "$root/var/lib/cassy-actions/cache/." \
        "$root/mnt/shockwave/home/pippenz/cassy-actions/cache/"
    write_mock "$root"
    printf '%s\n' "$root"
}

run_cutover() {
    local root="$1" output="$2"
    shift 2
    env PATH="$root/bin:$PATH" MOCK_ROOT="$root" \
        CASSY_ACTIONS_CUTOVER_TESTING=1 \
        CASSY_ACTIONS_CUTOVER_TEST_ROOT="$root" \
        CASSY_ACTIONS_CUTOVER_ARTIFACT_ROOT="$root/artifacts" \
        "$@" "$cutover" cutover >"$output" 2>&1
}

assert_original_state() {
    local root="$1" label="$2"
    [[ -d "$root/var/lib/cassy-actions/cache/cargo-target" ]] || fail "$label: root cache was not restored"
    assert_file_text "$root/state/mounted" 0 "$label mount"
    assert_file_text "$root/etc/fstab" 'rootfs / ext4 defaults 0 1' "$label fstab"
    assert_file_text "$root/etc/systemd/system/cassy-actions-runner.service" 'old unit 1' "$label unit 1"
    assert_file_text "$root/etc/systemd/system/cassy-actions-runner-2.service" 'old unit 2' "$label unit 2"
    assert_file_text "$root/state/services/cassy-actions-runner.service" active "$label service 1"
    assert_file_text "$root/state/services/cassy-actions-runner-2.service" active "$label service 2"
    assert_absent "$root/state/routes/CASSY_SELF_HOSTED_PILOT" "$label unset route"
    assert_file_text "$root/state/routes/CASSY_MERGE_QUEUE_SELF_HOSTED" enabled "$label merge route"
    assert_file_text "$root/state/routes/CASSY_RELEASE_SELF_HOSTED" enabled "$label release route"
}

test_injected_failure() {
    local stage="$1" root output status
    root="$(setup_fixture "failure-$stage")"
    output="$root/output.log"
    set +e
    run_cutover "$root" "$output" CASSY_ACTIONS_CUTOVER_TEST_FAIL_AFTER="$stage"
    status=$?
    set -e
    [[ "$status" -ne 0 ]] || fail "$stage injection unexpectedly succeeded"
    grep -q "injected failure after $stage" "$output" || fail "$stage injection was not observed"
    grep -q 'automatic rollback restored original host and route state' "$output" \
        || fail "$stage did not report successful restoration"
    ! grep -q 'cutover complete' "$output" || fail "$stage reported false success"
    assert_original_state "$root" "$stage"
    if [[ "$stage" == route-disable ]]; then
        ! grep -q '^stop ' "$root/state/systemctl.log" || fail 'route-disable failure stopped active services'
    fi
    passes=$((passes + 1))
    printf 'ok - injected failure after %s restores safely\n' "$stage"
}

for stage in route-disable source-rename mount service-restart; do
    test_injected_failure "$stage"
done

for fail_at in 1 2; do
    root="$(setup_fixture "route-set-failure-$fail_at")"
    set +e
    run_cutover "$root" "$root/output.log" MOCK_GH_SET_FAIL_AT="$fail_at"
    status=$?
    set -e
    [[ "$status" -ne 0 ]] || fail "route set failure $fail_at unexpectedly succeeded"
    ! grep -q '^stop ' "$root/state/systemctl.log" || fail "route set failure $fail_at stopped services"
    assert_original_state "$root" "route set failure $fail_at"
    passes=$((passes + 1))
    printf 'ok - route set failure %s propagates and restores exact routes\n' "$fail_at"
done

root="$(setup_fixture stop-failure-before-copy)"
set +e
run_cutover "$root" "$root/output.log" MOCK_SYSTEMCTL_STOP_FAIL_AT=1
status=$?
set -e
[[ "$status" -ne 0 ]] || fail 'partial service-stop failure unexpectedly succeeded'
grep -q 'automatic rollback restored original host and route state' "$root/output.log" \
    || fail 'partial service-stop failure did not restore service state'
[[ ! -s "$root/state/host-mutation.log" ]] || fail 'partial service-stop failure mutated host paths'
assert_original_state "$root" 'partial service-stop failure'
passes=$((passes + 1))
printf 'ok - partial service-stop failure restores both services before routes\n'

test_pre_rename_failure() {
    local name="$1"
    shift
    local root status
    root="$(setup_fixture "$name")"
    set +e
    run_cutover "$root" "$root/output.log" "$@"
    status=$?
    set -e
    [[ "$status" -ne 0 ]] || fail "$name unexpectedly succeeded"
    grep -q 'automatic rollback restored original host and route state' "$root/output.log" \
        || fail "$name did not restore stopped services"
    [[ ! -s "$root/state/host-mutation.log" ]] || fail "$name renamed or unmounted host data"
    assert_original_state "$root" "$name"
    passes=$((passes + 1))
    printf 'ok - %s restarts stopped services and restores routes\n' "$name"
}

test_pre_rename_failure rsync-failure MOCK_RSYNC_FAIL_AT=1
test_pre_rename_failure zero-diff-failure MOCK_RSYNC_DRY_DIFF=1
test_pre_rename_failure hash-failure MOCK_HASH_MISMATCH=1

root="$(setup_fixture wrong-destination-device)"
set +e
run_cutover "$root" "$root/output.log" MOCK_DEST_DEVICE=259:9
status=$?
set -e
[[ "$status" -ne 0 ]] || fail 'wrong destination device unexpectedly succeeded'
[[ ! -s "$root/state/rsync.log" ]] || fail 'wrong destination device reached rsync'
! grep -q 'variable set' "$root/state/gh.log" || fail 'wrong destination device changed routing'
assert_original_state "$root" 'wrong destination device'
passes=$((passes + 1))
printf 'ok - destination subtree and device are proven before destructive rsync\n'

root="$(setup_fixture escaped-destination-subtree)"
/bin/mv "$root/mnt/shockwave/home/pippenz/cassy-actions/cache" "$root/escaped-cache"
ln -s "$root/escaped-cache" "$root/mnt/shockwave/home/pippenz/cassy-actions/cache"
set +e
run_cutover "$root" "$root/output.log"
status=$?
set -e
[[ "$status" -ne 0 ]] || fail 'escaped destination subtree unexpectedly succeeded'
[[ ! -s "$root/state/rsync.log" ]] || fail 'escaped destination subtree reached rsync'
! grep -q 'variable set' "$root/state/gh.log" || fail 'escaped destination subtree changed routing'
passes=$((passes + 1))
printf 'ok - resolved destination cannot escape the exact Shockwave subtree\n'

root="$(setup_fixture busy-runner)"
set +e
run_cutover "$root" "$root/output.log" MOCK_RUNNER_BUSY=1
status=$?
set -e
[[ "$status" -ne 0 ]] || fail 'busy runner cutover unexpectedly succeeded'
! grep -q '^stop ' "$root/state/systemctl.log" || fail 'busy runner was stopped'
assert_original_state "$root" 'busy runner'
passes=$((passes + 1))
printf 'ok - pretty JSON busy=true never reaches service stop\n'

root="$(setup_fixture route-read-error)"
set +e
run_cutover "$root" "$root/output.log" MOCK_GH_GET_ERROR_NAME=CASSY_MERGE_QUEUE_SELF_HOSTED
status=$?
set -e
[[ "$status" -ne 0 ]] || fail 'route API failure was treated as unset'
! grep -q 'variable set' "$root/state/gh.log" || fail 'route API failure changed routing'
! grep -q '^stop ' "$root/state/systemctl.log" || fail 'route API failure stopped services'
assert_original_state "$root" 'route API failure'
passes=$((passes + 1))
printf 'ok - route 404 is distinct from API failure\n'

root="$(setup_fixture route-delete-error)"
set +e
run_cutover "$root" "$root/output.log" \
    CASSY_ACTIONS_CUTOVER_TEST_FAIL_AFTER=route-disable \
    MOCK_GH_DELETE_ERROR_NAME=CASSY_SELF_HOSTED_PILOT
status=$?
set -e
[[ "$status" -ne 0 ]] || fail 'route delete failure unexpectedly succeeded'
grep -q 'automatic rollback incomplete' "$root/output.log" || fail 'route delete failure was not explicit'
! grep -q 'automatic rollback restored' "$root/output.log" || fail 'route delete failure reported false restoration'
passes=$((passes + 1))
printf 'ok - route deletion failure is explicit and never false success\n'

root="$(setup_fixture wrong-bind-subtree)"
set +e
run_cutover "$root" "$root/output.log" MOCK_SOURCE_FSROOT=/wrong/cache
status=$?
set -e
[[ "$status" -ne 0 ]] || fail 'wrong bind subtree unexpectedly succeeded'
grep -q 'does not match Shockwave subtree' "$root/output.log" || fail 'wrong bind subtree was not diagnosed'
assert_original_state "$root" 'wrong bind subtree'
passes=$((passes + 1))
printf 'ok - exact Shockwave bind subtree is required\n'

root="$(setup_fixture busy-rollback)"
run_cutover "$root" "$root/cutover.log"
state_file="$(find "$root/artifacts" -maxdepth 1 -name 'operator-cutover-*.env' -type f | head -1)"
stops_before="$(grep -c '^stop ' "$root/state/systemctl.log")"
set +e
env PATH="$root/bin:$PATH" MOCK_ROOT="$root" MOCK_RUNNER_BUSY=1 \
    CASSY_ACTIONS_CUTOVER_TESTING=1 CASSY_ACTIONS_CUTOVER_TEST_ROOT="$root" \
    CASSY_ACTIONS_CUTOVER_ARTIFACT_ROOT="$root/artifacts" \
    "$cutover" rollback "$state_file" >"$root/rollback.log" 2>&1
status=$?
set -e
[[ "$status" -ne 0 ]] || fail 'busy manual rollback unexpectedly succeeded'
stops_after="$(grep -c '^stop ' "$root/state/systemctl.log")"
[[ "$stops_after" == "$stops_before" ]] || fail 'busy manual rollback stopped active services'
grep -q 'rollback incomplete' "$root/rollback.log" || fail 'busy manual rollback lacked recoverable-failure receipt'
! grep -q 'rollback complete from' "$root/rollback.log" || fail 'busy manual rollback reported false success'
passes=$((passes + 1))
printf 'ok - rollback refuses to stop an active job and reports recovery state\n'

root="$(setup_fixture rollback-stop-failure)"
run_cutover "$root" "$root/cutover.log"
state_file="$(find "$root/artifacts" -maxdepth 1 -name 'operator-cutover-*.env' -type f | head -1)"
set +e
env PATH="$root/bin:$PATH" MOCK_ROOT="$root" MOCK_SYSTEMCTL_STOP_FAIL_AT=2 \
    CASSY_ACTIONS_CUTOVER_TESTING=1 CASSY_ACTIONS_CUTOVER_TEST_ROOT="$root" \
    CASSY_ACTIONS_CUTOVER_ARTIFACT_ROOT="$root/artifacts" \
    "$cutover" rollback "$state_file" >"$root/rollback.log" 2>&1
status=$?
set -e
[[ "$status" -ne 0 ]] || fail 'rollback stop failure unexpectedly succeeded'
assert_file_text "$root/state/mounted" 1 'rollback stop failure mount'
[[ -d "$(awk -F= '/^ROLLBACK_PATH=/{print $2}' "$state_file")" ]] \
    || fail 'rollback stop failure renamed backup data'
! grep -q '^umount ' "$root/state/host-mutation.log" || fail 'rollback stop failure unmounted cache'
grep -q 'rollback incomplete' "$root/rollback.log" || fail 'rollback stop failure lacked recovery receipt'
! grep -q 'rollback complete from' "$root/rollback.log" || fail 'rollback stop failure reported false success'
passes=$((passes + 1))
printf 'ok - rollback stop failure blocks unmount and rename\n'

root="$(setup_fixture successful-round-trip)"
run_cutover "$root" "$root/cutover.log"
grep -q 'cutover complete' "$root/cutover.log" || fail 'successful fixture cutover lacked receipt'
state_file="$(find "$root/artifacts" -maxdepth 1 -name 'operator-cutover-*.env' -type f | head -1)"
[[ -n "$state_file" ]] || fail 'successful fixture cutover lacked state file'
env PATH="$root/bin:$PATH" MOCK_ROOT="$root" \
    CASSY_ACTIONS_CUTOVER_TESTING=1 CASSY_ACTIONS_CUTOVER_TEST_ROOT="$root" \
    CASSY_ACTIONS_CUTOVER_ARTIFACT_ROOT="$root/artifacts" \
    "$cutover" rollback "$state_file" >"$root/rollback.log" 2>&1
grep -q 'rollback complete' "$root/rollback.log" || fail 'manual rollback lacked success receipt'
assert_original_state "$root" 'manual rollback'
passes=$((passes + 1))
printf 'ok - successful fixture cutover rolls back mechanically\n'

printf '%d cutover safety behaviors passed\n' "$passes"

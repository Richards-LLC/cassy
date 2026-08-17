#!/usr/bin/env bash
set -euo pipefail

test_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
helper="$(cd "$test_dir/.." && pwd)/cas-update"
fake_kill="$test_dir/fake-kill"
failures=0
tests=0

pass() { tests=$((tests + 1)); printf 'ok %d - %s\n' "$tests" "$1"; }
fail() { tests=$((tests + 1)); failures=$((failures + 1)); printf 'not ok %d - %s\n' "$tests" "$1" >&2; }
assert_contains() { grep -Fq -- "$2" "$1" || { printf 'missing [%s] in %s\n' "$2" "$1" >&2; return 1; }; }
assert_not_contains() { ! grep -Fq -- "$2" "$1" || { printf 'unexpected [%s] in %s\n' "$2" "$1" >&2; return 1; }; }

make_binary() {
  local path="$1" version="$2" marker="$3"
  printf '#!/usr/bin/env bash\nif [ "${1:-}" = --version ]; then printf "%%s\\n" "%s"; exit; fi\nprintf "%%s\\n" "%s"\n' \
    "$version" "$marker" >"$path"
  chmod +x "$path"
}

make_proc() {
  local root="$1" pid="$2" exe="$3" behavior="$4" cmd="$5" env_data="${6:-}" i
  mkdir -p "$root/$pid"
  ln -s "$exe" "$root/$pid/exe"
  printf '%s (cas)' "$pid" >"$root/$pid/stat"
  for i in $(seq 3 21); do
    if [ "$i" -eq 3 ]; then printf ' S' >>"$root/$pid/stat"; else printf ' 1' >>"$root/$pid/stat"; fi
  done
  printf ' %s 0\n' "$((pid * 10))" >>"$root/$pid/stat"
  printf 'Name:\tcas\nPPid:\t1\n' >"$root/$pid/status"
  # shellcheck disable=SC2059
  printf "$cmd" >"$root/$pid/cmdline"
  # shellcheck disable=SC2059
  printf "$env_data" >"$root/$pid/environ"
  printf '%s\n' "$behavior" >"$root/$pid/behavior"
}

run_case() {
  local mode="$1" out="$2" tmp="$3"
  (
    export HOME="$tmp/home"
    export CAS_INSTALL="$tmp/bin/cas"
    export CAS_SRC="$tmp/source"
    export CAS_PROJECT_ROOTS="$tmp/home"
    export CAS_UPDATE_PROC_ROOT="$tmp/proc"
    export CAS_UPDATE_KILL_BIN="$fake_kill"
    export CAS_UPDATE_SIGNAL_LOG="$tmp/signals"
    export CAS_UPDATE_NEW_EXE="$tmp/new-cas"
    export CAS_UPDATE_SESSIONS_DIR="$tmp/home/.cas/sessions"
    export CAS_UPDATE_SERVER_REGISTRY_ROOTS="$tmp/registry"
    export CAS_UPDATE_WAIT_STEPS=1
    export CAS_UPDATE_WAIT_SLEEP=0
    export CAS_UPDATE_SOURCE_ONLY=1
    # shellcheck source=../cas-update
    source "$helper"
    SCAN_ROOTS="$tmp/home"
    ensure_state_files
    capture_installed_identity
    snapshot_old_processes
    install -m 0755 "$tmp/new-cas" "$CAS_INSTALL.new"
    mv -f "$CAS_INSTALL.new" "$CAS_INSTALL"
    NEW_INSTALLED_VERSION="$(binary_version "$CAS_INSTALL")"
    NEW_INSTALLED_HASH="$(sha256_file "$CAS_INSTALL")"
    case "$mode" in
      normal)
        turnover_old_processes
        verify_runtime_epoch
        ;;
      stale)
        if turnover_old_processes; then printf 'expected turnover failure\n' >&2; exit 1; fi
        if verify_runtime_epoch; then printf 'expected verification failure\n' >&2; exit 1; fi
        ;;
      dry-run)
        DRY_RUN=1
        turnover_old_processes
        ;;
      no-restart)
        DO_TURNOVER=0
        turnover_old_processes
        ;;
    esac
    MIGRATION_STATUS="complete"; SYNC_STATUS="complete"
    print_summary
    cleanup
  ) >"$out" 2>&1
}

new_fixture() {
  local tmp
  tmp="$(mktemp -d -t cas-update-test.XXXXXX)"
  mkdir -p "$tmp/home" "$tmp/bin" "$tmp/source" "$tmp/proc" "$tmp/registry"
  : >"$tmp/signals"
  make_binary "$tmp/bin/cas" 'cas 2.51.0 (old0001)' old
  cp "$tmp/bin/cas" "$tmp/old-running"
  make_binary "$tmp/new-cas" 'cas 2.52.0 (new0002)' new
  printf '%s' "$tmp"
}

make_build_fixture() {
  local tmp remote seed source tools
  tmp="$(new_fixture)"
  remote="$tmp/origin.git"; seed="$tmp/seed"; source="$tmp/source"; tools="$tmp/tools"
  rm -rf "$source"
  git init --bare --initial-branch=main "$remote" >/dev/null
  git init --initial-branch=main "$seed" >/dev/null
  git -C "$seed" config user.email cas-update-test@example.invalid
  git -C "$seed" config user.name 'cas-update test'
  mkdir -p "$seed/docs/release-notes"
  printf 'base tracked content\n' >"$seed/tracked.txt"
  git -C "$seed" add tracked.txt
  git -C "$seed" commit -m base >/dev/null
  git -C "$seed" remote add origin "$remote"
  git -C "$seed" push -u origin main >/dev/null 2>&1
  git clone "$remote" "$source" >/dev/null 2>&1
  git -C "$source" config user.email cas-update-test@example.invalid
  git -C "$source" config user.name 'cas-update test'
  git -C "$source" checkout -b operator-feature >/dev/null 2>&1
  mkdir -p "$source/docs/release-notes"
  mkdir -p "$source/.context/zig"
  printf '#!/usr/bin/env bash\n' >"$source/.context/zig/zig"
  chmod +x "$source/.context/zig/zig"
  printf 'operator branch only\n' >"$source/local-marker"
  git -C "$source" add local-marker
  git -C "$source" commit -m operator-feature >/dev/null
  printf 'operator dirty content\n' >"$source/tracked.txt"
  printf 'operator untracked release note\n' >"$source/docs/release-notes/2026-08-16-operator.md"

  printf 'upstream main content\n' >"$seed/upstream-marker"
  printf 'upstream release note\n' >"$seed/docs/release-notes/2026-08-16-operator.md"
  git -C "$seed" add upstream-marker docs/release-notes/2026-08-16-operator.md
  git -C "$seed" commit -m upstream-update >/dev/null
  git -C "$seed" push origin main >/dev/null 2>&1

  mkdir -p "$tools"
  printf '%s\n' '#!/usr/bin/env bash' 'set -euo pipefail' \
    'printf "%s\\n" "$PWD" >>"$CAS_UPDATE_CARGO_LOG"' \
    'printf "%s\\n" "${ZIG:-}" >>"$CAS_UPDATE_ZIG_LOG"' \
    'mkdir -p "$CARGO_TARGET_DIR/release-fast"' \
    'marker=checked-out' \
    '[ -f "$PWD/upstream-marker" ] && marker=fetched' \
    'printf "%s\\n" "#!/usr/bin/env bash" "if [ \"\${1:-}\" = --version ]; then printf \"cas build (%s)\\n\" \"$marker\"; fi" >"$CARGO_TARGET_DIR/release-fast/cas"' \
    'chmod +x "$CARGO_TARGET_DIR/release-fast/cas"' >"$tools/cargo"
  chmod +x "$tools/cargo"
  printf '%s' "$tmp"
}

run_build_case() {
  local tmp="$1" out="$2" pull="$3" force="$4"
  (
    export HOME="$tmp/home"
    export CAS_INSTALL="$tmp/bin/cas"
    export CAS_SRC="$tmp/source"
    export CAS_UPDATE_WORKTREE_ROOT="$tmp/build-worktrees"
    export CAS_UPDATE_CARGO_LOG="$tmp/cargo.log"
    export CAS_UPDATE_ZIG_LOG="$tmp/zig.log"
    export PATH="$tmp/tools:$PATH"
    unset ZIG
    # shellcheck source=../cas-update
    source "$helper"
    DO_PULL="$pull"; FORCE_BUILD="$force"; PROFILE=release
    build_and_install
  ) >"$out" 2>&1
}

test_build_uses_detached_fetched_worktree() {
  local tmp out remote_short stable_worktree
  tmp="$(make_build_fixture)"; out="$tmp/out"
  stable_worktree="$tmp/build-worktrees/cas-update-build"
  if run_build_case "$tmp" "$out" 1 1 \
    && [ "$(cat "$tmp/source/docs/release-notes/2026-08-16-operator.md")" = 'operator untracked release note' ] \
    && [ "$(git -C "$tmp/source" branch --show-current)" = operator-feature ] \
    && [ "$(cat "$tmp/source/tracked.txt")" = 'operator dirty content' ] \
    && [ "$(cat "$tmp/cargo.log")" != "$tmp/source" ] \
    && [ "$(cat "$tmp/cargo.log")" = "$stable_worktree" ] \
    && [ "$(cat "$tmp/zig.log")" = "$tmp/source/.context/zig/zig" ] \
    && assert_contains "$tmp/bin/cas" '"fetched"' \
    && git -C "$tmp/source" worktree list --porcelain | grep -Fq "$stable_worktree"; then
    pass 'fetched builds use a persistent detached worktree without touching dirty or untracked operator checkout files'
  else fail 'fetched builds use a persistent detached worktree without touching dirty or untracked operator checkout files'; fi

  printf 'second upstream content\n' >"$tmp/seed/second-upstream-marker"
  git -C "$tmp/seed" add second-upstream-marker
  git -C "$tmp/seed" commit -m second-upstream-update >/dev/null
  git -C "$tmp/seed" push origin main >/dev/null 2>&1
  : >"$tmp/cargo.log"
  if run_build_case "$tmp" "$out" 1 1 \
    && [ "$(cat "$tmp/cargo.log")" = "$stable_worktree" ] \
    && [ "$(git -C "$stable_worktree" rev-parse HEAD)" = "$(git -C "$tmp/seed" rev-parse main)" ]; then
    pass 'a later fetched commit reuses the same detached worktree path and refreshes it'
  else fail 'a later fetched commit reuses the same detached worktree path and refreshes it'; fi

  remote_short="$(git -C "$tmp/seed" rev-parse --short=7 main)"
  make_binary "$tmp/bin/cas" "cas build ($remote_short)" current
  : >"$tmp/cargo.log"
  if run_build_case "$tmp" "$out" 1 0 \
    && [ ! -s "$tmp/cargo.log" ] \
    && assert_contains "$out" 'already at fetched origin/main; skipping build'; then
    pass 'installed-hash short-circuit compares the fetched origin commit, not the operator checkout HEAD'
  else fail 'installed-hash short-circuit compares the fetched origin commit, not the operator checkout HEAD'; fi

  : >"$tmp/cargo.log"
  if run_build_case "$tmp" "$out" 0 1 \
    && [ "$(cat "$tmp/cargo.log")" = "$tmp/source" ] \
    && assert_contains "$tmp/bin/cas" '"checked-out"'; then
    pass '--no-pull still builds the current checked-out content directly'
  else fail '--no-pull still builds the current checked-out content directly'; fi

  git -C "$tmp/source" worktree lock "$stable_worktree" --reason test-lock
  : >"$tmp/cargo.log"
  if run_build_case "$tmp" "$out" 1 1 \
    && [ "$(cat "$tmp/cargo.log")" = "$stable_worktree" ] \
    && ! git -C "$tmp/source" worktree list --porcelain | awk -v path="$stable_worktree" '
      $1 == "worktree" { active = ($2 == path); next }
      active && $1 == "locked" { found = 1 }
      END { exit !found }
    '; then
    pass 'a locked stable worktree is recreated and the fetched build continues'
  else fail 'a locked stable worktree is recreated and the fetched build continues'; fi

  rm -f "$stable_worktree/.git"
  : >"$tmp/cargo.log"
  if run_build_case "$tmp" "$out" 1 1 \
    && [ "$(cat "$tmp/cargo.log")" = "$stable_worktree" ] \
    && [ "$(git -C "$stable_worktree" rev-parse --is-inside-work-tree)" = true ]; then
    pass 'a corrupted stable worktree is recreated and the fetched build continues'
  else fail 'a corrupted stable worktree is recreated and the fetched build continues'; fi
  rm -rf "$tmp"
}

test_graceful_and_forced() {
  local tmp out
  tmp="$(new_fixture)"; out="$tmp/out"
  make_proc "$tmp/proc" 101 "$tmp/old-running" reconnect 'cas\0serve\0' 'CAS_AGENT_NAME=client-a\0CAS_FACTORY_SESSION=alpha\0'
  make_proc "$tmp/proc" 102 "$tmp/old-running" stubborn 'cas\0factory\0' 'CAS_AGENT_NAME=director\0CAS_FACTORY_SESSION=beta\0'
  make_proc "$tmp/proc" 103 "$tmp/new-cas" survivor 'cas\0serve\0' ''
  if run_case normal "$out" "$tmp" \
    && assert_contains "$tmp/signals" 'TERM 101' \
    && assert_contains "$tmp/signals" 'TERM 102' \
    && assert_contains "$tmp/signals" 'KILL 102' \
    && assert_not_contains "$tmp/signals" '103' \
    && assert_contains "$out" 'gracefully stopped (SIGTERM)' \
    && assert_contains "$out" 'FORCIBLY KILLED (SIGKILL)' \
    && assert_contains "$out" 'reconnected and verified installed hash at PID 101' \
    && assert_contains "$out" 'detached-factory / beta / director' \
    && assert_contains "$out" 'verified: no process executes the previous installed hash'; then
    pass 'exact old epoch gets graceful then narrow forced turnover; new binary is untouched'
  else fail 'exact old epoch gets graceful then narrow forced turnover; new binary is untouched'; fi
  rm -rf "$tmp"
}

test_ownership_classification() {
  local tmp out
  tmp="$(new_fixture)"; out="$tmp/out"
  mkdir -p "$tmp/home/.cas/sessions"
  printf '{\n  "name": "factory-one",\n  "daemon_pid": 201\n}\n' >"$tmp/home/.cas/sessions/factory-one.json"
  printf '{\n  "name": "api",\n  "command": "cas serve",\n  "cwd": "/work/api",\n  "pid": 202,\n  "owner_task": "cas-abcd",\n  "owner_worker": "worker-a",\n  "factory_session": "factory-two"\n}\n' >"$tmp/registry/srv-api.json"
  make_proc "$tmp/proc" 201 "$tmp/old-running" graceful 'cas\0factory\0' ''
  make_proc "$tmp/proc" 202 "$tmp/old-running" graceful 'cas\0serve\0' ''
  if run_case normal "$out" "$tmp" \
    && assert_contains "$out" 'factory-daemon / factory-one / factory:factory-one' \
    && assert_contains "$out" 'registered-server / factory-two / worker-a/cas-abcd' \
    && assert_contains "$out" 'coordination server_start from /work/api: cas serve'; then
    pass 'factory and registered server records drive explicit owner/restart summaries'
  else fail 'factory and registered server records drive explicit owner/restart summaries'; fi
  rm -rf "$tmp"
}

test_dry_run_and_opt_out() {
  local tmp out
  tmp="$(new_fixture)"; out="$tmp/out"
  make_proc "$tmp/proc" 301 "$tmp/old-running" survivor 'cas\0serve\0' ''
  if run_case dry-run "$out" "$tmp" \
    && [ ! -s "$tmp/signals" ] \
    && assert_contains "$out" 'planned SIGTERM; fingerprint-gated SIGKILL only if needed' \
    && assert_contains "$out" 'dry-run (turnover planned, no signals sent)'; then
    pass '--dry-run is non-mutating and displays exact turnover plan'
  else fail '--dry-run is non-mutating and displays exact turnover plan'; fi
  rm -rf "$tmp"

  tmp="$(new_fixture)"; out="$tmp/out"
  make_proc "$tmp/proc" 302 "$tmp/old-running" survivor 'cas\0serve\0' ''
  if run_case no-restart "$out" "$tmp" \
    && [ ! -s "$tmp/signals" ] \
    && assert_contains "$out" 'skipped by explicit opt-out' \
    && assert_contains "$out" 'not stopped (explicit opt-out)'; then
    pass '--no-restart leaves runtimes untouched and reports the opt-out'
  else fail '--no-restart leaves runtimes untouched and reports the opt-out'; fi
  rm -rf "$tmp"
}

test_stale_survivor_is_nonzero() {
  local tmp out
  tmp="$(new_fixture)"; out="$tmp/out"
  make_proc "$tmp/proc" 401 "$tmp/old-running" survivor 'cas\0serve\0' ''
  if run_case stale "$out" "$tmp" \
    && assert_contains "$tmp/signals" 'TERM 401' \
    && assert_contains "$tmp/signals" 'KILL 401' \
    && assert_contains "$out" 'FAILED: 1 stale/unexpected CAS process epoch(s) remain' \
    && assert_contains "$out" 'STALE CAS PID 401 remains'; then
    pass 'stale old-version survivor fails turnover and final verification loudly'
  else fail 'stale old-version survivor fails turnover and final verification loudly'; fi
  rm -rf "$tmp"
}

test_no_process_and_flag_semantics() {
  local tmp out flags
  tmp="$(new_fixture)"; out="$tmp/out"
  if run_case normal "$out" "$tmp" \
    && assert_contains "$out" 'verified: no process executes the previous installed hash' \
    && assert_contains "$out" 'CAS processes stopped or killed:' \
    && assert_contains "$out" '    none'; then
    pass 'no-running-process case prints an explicit none summary'
  else fail 'no-running-process case prints an explicit none summary'; fi
  rm -rf "$tmp"

  flags="$(CAS_UPDATE_SOURCE_ONLY=1 bash -c '
    source "$1"
    parse_args --build-only
    printf "build=%s sync=%s turnover=%s\\n" "$DO_BUILD" "$DO_SYNC" "$DO_TURNOVER"
    DO_BUILD=1; DO_SYNC=1; DO_TURNOVER=1
    parse_args --sync-only
    printf "build=%s sync=%s turnover=%s\\n" "$DO_BUILD" "$DO_SYNC" "$DO_TURNOVER"
    DO_BUILD=1; DO_SYNC=1; DO_TURNOVER=1
    parse_args --no-restart
    printf "build=%s sync=%s turnover=%s\\n" "$DO_BUILD" "$DO_SYNC" "$DO_TURNOVER"
  ' _ "$helper")"
  if [ "$flags" = $'build=1 sync=0 turnover=0\nbuild=0 sync=1 turnover=0\nbuild=1 sync=1 turnover=0' ]; then
    pass 'build-only, sync-only, and no-restart have explicit non-overlapping semantics'
  else fail 'build-only, sync-only, and no-restart have explicit non-overlapping semantics'; fi
}

test_installer() {
  local tmp
  tmp="$(mktemp -d -t cas-update-install.XXXXXX)"
  if CAS_UPDATE_INSTALL_DIR="$tmp/bin" "$test_dir/../install.sh" >/dev/null \
    && cmp -s "$helper" "$tmp/bin/cas-update" \
    && [ -x "$tmp/bin/cas-update" ]; then
    pass 'tracked helper installer copies executable to the requested bin directory'
  else fail 'tracked helper installer copies executable to the requested bin directory'; fi
  rm -rf "$tmp"
}

test_main_exit_and_help() {
  local tmp out
  tmp="$(new_fixture)"; out="$tmp/out"
  if "$helper" --help >"$out" \
    && assert_contains "$out" 'cas-update --no-restart' \
    && HOME="$tmp/home" CAS_INSTALL="$tmp/bin/cas" CAS_PROJECT_ROOTS="$tmp/home" \
       CAS_UPDATE_PROC_ROOT="$tmp/proc" CAS_UPDATE_SESSIONS_DIR="$tmp/home/.cas/sessions" \
       CAS_UPDATE_SERVER_REGISTRY_ROOTS="$tmp/registry" \
       "$helper" --sync-only --projects "$tmp/home" >"$out" 2>&1 \
    && assert_contains "$out" 'Previous installed: cas 2.51.0 (old0001)' \
    && assert_contains "$out" 'Runtime turnover:   skipped by explicit opt-out'; then
    pass 'top-level help and sync-only main path exit zero with the binding summary'
  else fail 'top-level help and sync-only main path exit zero with the binding summary'; fi
  rm -rf "$tmp"
}

test_updater_ancestry_is_protected() {
  local tmp out
  tmp="$(new_fixture)"; out="$tmp/out"
  make_proc "$tmp/proc" "$$" "$tmp/old-running" survivor 'cas\0serve\0' ''
  if (
    export HOME="$tmp/home" CAS_INSTALL="$tmp/bin/cas" CAS_PROJECT_ROOTS="$tmp/home"
    export CAS_UPDATE_PROC_ROOT="$tmp/proc" CAS_UPDATE_SOURCE_ONLY=1
    export CAS_UPDATE_SESSIONS_DIR="$tmp/home/.cas/sessions"
    export CAS_UPDATE_SERVER_REGISTRY_ROOTS="$tmp/registry"
    source "$helper"
    SCAN_ROOTS="$tmp/home"
    ensure_state_files; capture_installed_identity; snapshot_old_processes
    ! grep -q "^$$|" "$OLD_PROCESS_FILE"
    cleanup
  ) >"$out" 2>&1; then
    pass 'the updater shell and its ancestry are excluded from the frozen signal plan'
  else fail 'the updater shell and its ancestry are excluded from the frozen signal plan'; fi
  rm -rf "$tmp"
}

test_graceful_and_forced
test_ownership_classification
test_dry_run_and_opt_out
test_stale_survivor_is_nonzero
test_no_process_and_flag_semantics
test_installer
test_updater_ancestry_is_protected
test_main_exit_and_help
test_build_uses_detached_fetched_worktree

if [ "$failures" -ne 0 ]; then
  printf '%s of %s tests failed\n' "$failures" "$tests" >&2
  exit 1
fi
printf 'all %s tests passed\n' "$tests"

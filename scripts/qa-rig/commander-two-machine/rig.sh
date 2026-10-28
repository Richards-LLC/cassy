#!/usr/bin/env bash
# Reusable two-machine Commander QA rig: a candidate `cas hub serve` on each of
# two real machines, each with its own saved state, beside (never instead of)
# the operator's live hub. See README.md beside this script.
#
#   rig.sh start     start this machine's rig daemon + hub, then the remote's
#   rig.sh stop      stop both; free the ports and Tailscale Serve mappings, keep state
#   rig.sh status    show both machines' rig processes and serve mappings
#   rig.sh local-start | local-stop | local-status   (run on one machine; used over ssh)
#   rig.sh pair <controller-origin>   mint this machine's one-time invitation (prints JSON)
#
# Environment: RIG (state dir, default ~/.cas/qa-rig/commander-two-machine; keep it
# short, the daemon's Unix socket $RIG/home/.cas/factory-*.sock must fit 104 bytes),
# REMOTE (ssh host, default prowl), RIG_HUB_PORT (4273), RIG_TS_PORT (8443).
# The rig binary is $RIG/bin/cas: copy the candidate there before `start`.
set -euo pipefail

RIG="${RIG:-$HOME/.cas/qa-rig/commander-two-machine}"
REMOTE="${REMOTE:-prowl}"
HUB_PORT="${RIG_HUB_PORT:-4273}"
TS_PORT="${RIG_TS_PORT:-8443}"
SESSION="commander-rig"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || pwd)"

# The rig's own HOME: its hub state is $RIG/home/.cas/hub, never ~/.cas/hub.
rig_env() {
    # Drop every inherited Cassy variable: a factory shell's CAS_ROOT or
    # CAS_FACTORY_SESSION would point the rig daemon at a live store.
    local name
    for name in $(compgen -e | grep '^CAS_' || true); do unset "$name"; done
    export HOME="$RIG/home"
    export PATH="$RIG/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
    export CAS_SKIP_FACTORY_TOOLING=1 CAS_SUPPRESS_DUPLICATE_WARNING=1
    export TERM=xterm-256color LANG=C.UTF-8
}

alive() { [[ -f "$1" ]] && kill -0 "$(cat "$1")" 2>/dev/null; }

local_init() {
    mkdir -p "$RIG/bin" "$RIG/home" "$RIG/project" "$RIG/logs" "$RIG/run"
    chmod 700 "$RIG" "$RIG/home"
    [[ -x "$RIG/bin/cas" ]] || { echo "rig: copy the candidate binary to $RIG/bin/cas first" >&2; exit 2; }
    # An inert supervisor: the real daemon owns the queue and history.
    if [[ ! -x "$RIG/bin/claude" ]]; then
        cat > "$RIG/bin/claude" <<'PY'
#!/usr/bin/env python3
import sys, time
if '--version' in sys.argv: print('2.1.47'); sys.exit(0)
if '--help' in sys.argv: print('--dangerously-skip-permissions --model --agent --mcp-config'); sys.exit(0)
print('Claude Code commander rig supervisor\n> ', flush=True)
while True:
    sys.stdin.readline(); time.sleep(.1)
PY
        chmod 700 "$RIG/bin/claude"
    fi
    if [[ ! -d "$RIG/project/.git" ]]; then
        ( rig_env; cd "$RIG/project"; git init -q .; cas init --yes --no-integrations >/dev/null
          printf '[factory.ai_enrichment]\nenabled = false\n' > .cas/config.toml )
    fi
}

local_start() {
    local_init
    ( rig_env; cd "$RIG/project"
      if ! alive "$RIG/run/daemon.pid"; then
          nohup cas factory daemon --session "$SESSION" --cwd "$RIG/project" --workers 0 \
              --no-worktrees --no-phone-home --foreground --supervisor-name rig-supervisor \
              >> "$RIG/logs/daemon.log" 2>&1 &
          echo $! > "$RIG/run/daemon.pid"
      fi
      if ! alive "$RIG/run/hub.pid"; then
          nohup cas hub serve --port "$HUB_PORT" --tailscale-serve --tailscale-serve-port "$TS_PORT" \
              >> "$RIG/logs/hub.log" 2>&1 &
          echo $! > "$RIG/run/hub.pid"
      fi
      for _ in $(seq 1 150); do
          curl -fsS "http://127.0.0.1:$HUB_PORT/v1/health" >/dev/null 2>&1 && break
          sleep 0.2
      done
      curl -fsS "http://127.0.0.1:$HUB_PORT/v1/health" >/dev/null || { echo "rig: hub did not start; see $RIG/logs/hub.log" >&2; exit 1; } )
    local_status
}

stop_pid() {
    local file="$1"
    if alive "$file"; then
        local pid; pid="$(cat "$file")"
        kill -TERM "$pid" 2>/dev/null || true
        for _ in $(seq 1 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.2; done
        kill -0 "$pid" 2>/dev/null && kill -KILL "$pid" 2>/dev/null || true
    fi
    rm -f "$file"
}

local_stop() {
    # The hub removes its own :$TS_PORT mapping on a clean stop; turn it off
    # again only on that port, never the live hub's.
    stop_pid "$RIG/run/hub.pid"
    stop_pid "$RIG/run/daemon.pid"
    if tailscale serve status 2>/dev/null | grep -q ":$TS_PORT"; then
        tailscale serve --https="$TS_PORT" off >/dev/null 2>&1 || true
    fi
    local_status
}

local_status() {
    local host; host="$(hostname -s)"
    for name in daemon hub; do
        if alive "$RIG/run/$name.pid"; then echo "$host $name: running (pid $(cat "$RIG/run/$name.pid"))"
        else echo "$host $name: stopped"; fi
    done
    if tailscale serve status 2>/dev/null | grep -q ":$TS_PORT"; then echo "$host serve :$TS_PORT: mapped"
    else echo "$host serve :$TS_PORT: none"; fi
    [[ -f "$RIG/home/.cas/hub/commander-usage.json" ]] && echo "$host ledger: $RIG/home/.cas/hub/commander-usage.json"
    return 0
}

# The remote runs this same script from stdin, with its own $HOME-relative rig.
remote() { ssh -o BatchMode=yes "$REMOTE" "PATH=/opt/homebrew/bin:/usr/local/bin:\$PATH RIG=\"\$HOME/.cas/qa-rig/commander-two-machine\" bash -s -- $1" < "$HERE/rig.sh"; }

case "${1:-}" in
    local-start) local_start ;;
    local-stop) local_stop ;;
    local-status) local_status ;;
    pair)
        [[ -n "${2:-}" ]] || { echo "usage: rig.sh pair <controller-origin>" >&2; exit 2; }
        ( rig_env; cd "$RIG/project"
          host="$(tailscale status --self --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))')"
          cas --json hub pair --origin "$2" --hub-url "https://$host:$TS_PORT" ) ;;
    start) local_start; remote local-start ;;
    stop) local_stop; remote local-stop ;;
    status) local_status; remote local-status ;;
    *) sed -n '2,15p' "$0"; exit 2 ;;
esac

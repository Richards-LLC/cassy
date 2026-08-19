#!/usr/bin/env bash
# Start the cache server outside GitHub's per-step process tracking, then
# replace this wrapper with the listener. Starting sccache inside a workflow
# step makes Runner.Worker reap it before the next step.
set -euo pipefail

sccache --start-server
sccache --show-stats >/dev/null
# GitHub's service wrapper translates systemd SIGTERM to Runner.Listener
# SIGINT, allowing the remote session to close cleanly. The interactive run.sh
# loop can leave a stale session that rejects the restarted listener.
exec /var/lib/cassy-actions/runner/bin/runsvc.sh

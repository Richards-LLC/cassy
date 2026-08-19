#!/usr/bin/env bash
# Start the cache server outside GitHub's per-step process tracking, then
# replace this wrapper with the listener. Starting sccache inside a workflow
# step makes Runner.Worker reap it before the next step.
set -euo pipefail

sccache --start-server
sccache --show-stats >/dev/null
exec /var/lib/cassy-actions/runner/run.sh


#!/usr/bin/env bash
# Slot 2 owns a distinct sccache server and GitHub listener. Keep both outside
# Runner.Worker's per-step process tracking so they survive between jobs.
set -euo pipefail

sccache --start-server
sccache --show-stats >/dev/null
exec /var/lib/cassy-actions/runner-2/bin/runsvc.sh

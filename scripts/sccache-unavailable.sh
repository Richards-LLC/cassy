#!/usr/bin/env bash
# Keep mozilla-actions/sccache-action's post hook harmless after its setup
# failed. The hook asks its configured executable for both human and JSON
# stats, so `/usr/bin/true` alone would still fail JSON parsing.
set -euo pipefail

if [[ " $* " == *" --stats-format=json "* ]]; then
    printf '%s\n' '{"stats":{"compile_requests":0,"requests_executed":0,"cache_errors":{"counts":{},"adv_counts":{}},"cache_hits":{"counts":{},"adv_counts":{}},"cache_misses":{"counts":{},"adv_counts":{}},"cache_write_errors":0,"cache_writes":0,"cache_write_duration":{"secs":0,"nanos":0},"cache_read_hit_duration":{"secs":0,"nanos":0},"compiler_write_duration":{"secs":0,"nanos":0}}}'
else
    printf '%s\n' 'sccache unavailable; build ran uncached'
fi

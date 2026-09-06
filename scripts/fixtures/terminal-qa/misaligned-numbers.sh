#!/bin/sh
# terminal-qa fixture: a numeric column that is left-aligned.
. "$(dirname "$0")/common.sh"
printf 'summary: 3 phases\n'
printf '%-12s  %-8s  %s\n' "phase" "ms" "note"
printf '%-12s  %-8s  %s\n' "store" "12" "fast"
printf '%-12s  %-8s  %s\n' "index" "1350" "slow"
printf '%-12s  %-8s  %s\n' "cloud" "7" "fast"

#!/bin/sh
# terminal-qa fixture: cells end in an ellipsis and nothing names --full.
. "$(dirname "$0")/common.sh"
printf 'summary: 2 roots\n'
printf '%-8s  %s\n' "name" "path"
printf '%-8s  %s\n' "alpha" "/home/example/projects/alpha/checkouts/wor$ELLIPSIS"
printf '%-8s  %s\n' "beta" "/home/example/projects/beta/checkouts/wor$ELLIPSIS"

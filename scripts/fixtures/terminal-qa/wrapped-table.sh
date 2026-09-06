#!/bin/sh
# terminal-qa fixture: one table row is 95 cells wide and wraps at 80 columns.
. "$(dirname "$0")/common.sh"
printf 'summary: 2 rows\n'
printf '%-12s  %8s  %6s  %s\n' "name" "size" "age" "path"
printf '%-12s  %8s  %6s  %s\n' "alpha" "12 KiB" "3d" "/home/example/projects/alpha"
printf '%-12s  %8s  %6s  %s\n' "beta-service" "1024 KiB" "14d" "/home/example/projects/very/deep/directory/tree/beta-service/checkout"

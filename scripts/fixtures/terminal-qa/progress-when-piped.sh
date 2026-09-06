#!/bin/sh
# terminal-qa fixture: spinner redraws and erase sequences even when piped.
. "$(dirname "$0")/common.sh"
printf 'indexing 1/3\r'
printf '\033[2Kindexing 2/3\r'
printf '\033[2Kindexing 3/3\n'
printf 'done\n'

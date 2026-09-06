#!/bin/sh
# terminal-qa fixture: box drawing and a check mark regardless of locale.
. "$(dirname "$0")/common.sh"
printf '✓ healthy\n'
printf '────────────────────────────────────────\n'
printf 'store │ ok\n'

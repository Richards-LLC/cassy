#!/bin/sh
# terminal-qa fixture: --json prints a log line on stdout before the document.
. "$(dirname "$0")/common.sh"
if [ "${1:-}" = "--json" ]; then
  printf 'loading store\n'
  printf '{"verdict":"healthy","checks":3}\n'
  exit 0
fi
printf 'healthy: 3 checks\n'

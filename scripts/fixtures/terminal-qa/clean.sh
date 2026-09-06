#!/bin/sh
# terminal-qa fixture: a well-behaved status command. Must PASS every check.
. "$(dirname "$0")/common.sh"

if [ "${1:-}" = "--json" ]; then
  printf '{"verdict":"healthy","checks":3,"warnings":0,"elapsed_ms":135}\n'
  exit 0
fi

printf '%s%s%s %shealthy%s %s 3 checks %s 0 warnings %s 135ms\n' "$GREEN" "$OK" "$RESET" "$BOLD" "$RESET" "$DOT" "$DOT" "$DOT"
tqa_rule
printf '%-14s %10s  %s\n' "Check" "Duration" "Result"
printf '%-14s %10s  %s%s%s ok\n' "store" "12ms" "$GREEN" "$OK" "$RESET"
printf '%-14s %10s  %s%s%s ok\n' "index" "3ms" "$GREEN" "$OK" "$RESET"
printf '%-14s %10s  %s%s%s ok\n' "cloud" "120ms" "$GREEN" "$OK" "$RESET"
printf '\n'
printf 'root     /home/example/projects/very-long-name%s  (--full shows the path)\n' "$ELLIPSIS"
printf 'receipt  clean tree at 9aeb3e1\n'

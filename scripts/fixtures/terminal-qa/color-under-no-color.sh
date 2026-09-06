#!/bin/sh
# terminal-qa fixture: styling that ignores NO_COLOR (but respects the tty check).
. "$(dirname "$0")/common.sh"
if [ "$TQA_TTY" = 1 ]; then
  printf '\033[1;31m!\033[0m 1 failing check\n'
else
  printf '! 1 failing check\n'
fi
printf 'store  ok\n'

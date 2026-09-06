#!/bin/sh
# terminal-qa fixture: truecolor body text too close to the background.
# #464646 vanishes on dark palettes; #dcdcdc vanishes on light ones.
. "$(dirname "$0")/common.sh"
if [ "$TQA_COLOR" = 1 ]; then
  printf '\033[38;2;70;70;70mquiet text that should be readable\033[0m\n'
  printf '\033[38;2;220;220;220mloud text that should be readable\033[0m\n'
else
  printf 'quiet text that should be readable\n'
  printf 'loud text that should be readable\n'
fi
printf 'done\n'

# Shared helpers for terminal-qa fixtures. Sourced, never executed.
#
# Sets: TQA_TTY (1 when stdout is a terminal), TQA_COLS (pty width or 80),
# TQA_UTF (1 when the locale is UTF-8), TQA_COLOR (1 when styling is allowed:
# a tty and NO_COLOR unset), and the glyph/SGR variables the fixtures print.

if [ -t 1 ]; then TQA_TTY=1; else TQA_TTY=0; fi
TQA_COLS=""
if [ "$TQA_TTY" = 1 ]; then
  TQA_COLS=$(stty size 2>/dev/null | awk '{print $2}')
  [ -z "$TQA_COLS" ] && TQA_COLS=$(tput cols 2>/dev/null)
fi
[ -z "$TQA_COLS" ] && TQA_COLS=80

case "${LC_ALL:-${LC_CTYPE:-${LANG:-}}}" in
  *UTF-8*|*utf-8*|*UTF8*|*utf8*) TQA_UTF=1 ;;
  *) TQA_UTF=0 ;;
esac

if [ "$TQA_TTY" = 1 ] && [ -z "${NO_COLOR:-}" ]; then TQA_COLOR=1; else TQA_COLOR=0; fi

if [ "$TQA_UTF" = 1 ]; then
  OK='✓'; WARN='⚠'; FAIL='✗'; RULE='─'; ELLIPSIS='…'; DOT='·'
else
  OK='[OK]'; WARN='[WARN]'; FAIL='[ERR]'; RULE='-'; ELLIPSIS='...'; DOT='-'
fi

if [ "$TQA_COLOR" = 1 ]; then
  # Marks only, never body text. Red: bold ANSI red survives every palette.
  # Green: ANSI green fails 3:1 on VS Code Light (#00bc00), so a fixed
  # mid-tone truecolor green is used instead.
  RED=$(printf '\033[1;31m'); GREEN=$(printf '\033[1;38;2;31;143;63m'); BOLD=$(printf '\033[1m'); RESET=$(printf '\033[0m')
else
  RED=''; GREEN=''; BOLD=''; RESET=''
fi

tqa_rule() {
  # A rule as wide as the terminal, capped at 80.
  n=$TQA_COLS
  [ "$n" -gt 80 ] && n=80
  i=0
  while [ "$i" -lt "$n" ]; do printf '%s' "$RULE"; i=$((i + 1)); done
  printf '\n'
}

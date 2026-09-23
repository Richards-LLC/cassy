#!/bin/sh
if [ -f "$HOME/slow-start" ]; then /bin/sleep 2; fi
case "$*" in
  'status --json') printf '%s' '{"Self":{"DNSName":"slow.tail.example."}}' ;;
  'serve status --json')
    if [ -f "$HOME/mock-serve" ]; then
      target=$(/bin/cat "$HOME/mock-serve")
      printf '{"Web":{"slow.tail.example:443":{"Handlers":{"/":{"Proxy":"%s"}}}}}' "$target"
    else printf '%s' '{}'; fi ;;
  'serve --bg --yes --https=443 '*) printf '%s' "$5" > "$HOME/mock-serve" ;;
  'serve --https=443 off') /bin/rm -f "$HOME/mock-serve" ;;
  *) exit 9 ;;
esac

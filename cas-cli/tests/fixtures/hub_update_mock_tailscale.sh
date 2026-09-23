#!/bin/sh
case "$*" in
  'status --json')
    printf '%s' '{"Self":{"DNSName":"localhost."}}'
    ;;
  'serve status --json')
    if [ -f "$HOME/mock-route" ]; then
      port=$(/bin/cat "$HOME/mock-port")
      target=$(/bin/cat "$HOME/mock-route")
      printf '{"Web":{"localhost:%s":{"Handlers":{"/":{"Proxy":"%s"}}}}}' "$port" "$target"
    else
      printf '%s' '{}'
    fi
    ;;
  'serve --bg --yes --https='*)
    printf '%s' "$5" > "$HOME/mock-route"
    ;;
  'serve --https='*' off')
    /bin/rm -f "$HOME/mock-route"
    ;;
  *)
    exit 9
    ;;
esac

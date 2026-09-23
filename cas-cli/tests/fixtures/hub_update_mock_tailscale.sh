#!/bin/sh
case "$*" in
  'status --json')
    dns_name=localhost.
    if [ -f "$HOME/mock-dns-name" ]; then
      dns_name=$(/bin/cat "$HOME/mock-dns-name")
    fi
    printf '{"Self":{"DNSName":"%s"}}' "$dns_name"
    ;;
  'serve status --json')
    if [ -f "$HOME/mock-route" ]; then
      port=$(/bin/cat "$HOME/mock-port")
      target=$(/bin/cat "$HOME/mock-route")
      dns_name=localhost.
      if [ -f "$HOME/mock-dns-name" ]; then
        dns_name=$(/bin/cat "$HOME/mock-dns-name")
      fi
      printf '{"Web":{"%s:%s":{"Handlers":{"/":{"Proxy":"%s"}}}}}' "${dns_name%.}" "$port" "$target"
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

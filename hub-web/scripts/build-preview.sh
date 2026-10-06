#!/usr/bin/env bash
# Build the clickable Commander preview: the committed production bundle
# (hub-web/dist) at /commander/, plus an in-page fixture hub (preview/).
# Output: hub-web/preview-dist/ — a static site. Serve it and open /commander/.
set -euo pipefail
hub="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rm -rf "$hub/preview-dist"
mkdir -p "$hub/preview-dist/commander"
cp -r "$hub/dist/." "$hub/preview-dist/commander/"
(cd "$hub" && npx vite build --config vite.preview.config.ts --logLevel warn)
python3 - "$hub/preview-dist/commander/index.html" <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
shim = '<script type="module" src="/commander/preview/shim.js"></script>'
s = s.replace('<script type="module" crossorigin src="/commander/app.js"></script>', shim)
s = s.replace('<title>Cassy Cloud</title>', '<title>Cassy Commander preview</title>')
open(p, 'w').write(s)
PY
printf '<!doctype html><meta http-equiv="refresh" content="0; url=/commander/">\n' > "$hub/preview-dist/index.html"
echo "preview: $hub/preview-dist (open /commander/)"

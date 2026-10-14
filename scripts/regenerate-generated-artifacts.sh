#!/usr/bin/env bash
# cas-0c988: rebuild committed build output after a merge instead of hand-merging it.
#
# Usage: scripts/regenerate-generated-artifacts.sh <pre-merge-head> <merged-head>
# Run in the merged checkout (Cassy's merge does this; so can a supervisor after
# a manual `git merge`). .gitattributes marks hub-web/dist `merge=cas-generated`,
# so the merge kept the target's copy without a conflict. This rebuilds it from
# the merged sources and commits it. No hub-web input changed: nothing to do.
set -euo pipefail
old=${1:?pre-merge head}
new=${2:?merged head}

if git diff --quiet "$old" "$new" -- hub-web ':(exclude)hub-web/dist'; then
  exit 0
fi

cd hub-web
# A merge may run in an ephemeral worktree with no node_modules: reuse the
# main checkout's when its lockfile matches, otherwise install from the lockfile.
if [ ! -e node_modules ]; then
  main=$(git worktree list --porcelain | sed -n '1s/^worktree //p')
  if [ -n "$main" ] && [ -d "$main/hub-web/node_modules" ] \
    && cmp -s package-lock.json "$main/hub-web/package-lock.json"; then
    ln -s "$main/hub-web/node_modules" node_modules
  else
    npm ci --prefer-offline --no-audit --no-fund >/dev/null
  fi
fi
npm run build >/dev/null
cd ..

if [ -n "$(git status --porcelain -- hub-web/dist)" ]; then
  git add -A hub-web/dist
  git commit -q \
    -m "build(hub-web): regenerate dist from merged sources" \
    -m "Rebuilt by scripts/regenerate-generated-artifacts.sh after merging ${new:0:9} (cas-0c988)."
  echo "regenerated hub-web/dist"
fi

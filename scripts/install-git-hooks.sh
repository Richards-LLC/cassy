#!/usr/bin/env bash
# Operator-reviewed main-checkout installation. Worker guards belong to cas factory.
set -euo pipefail

refuse() { printf 'Git hooks NOT installed: %s\n' "$*" >&2; exit 1; }
for marker in CAS_AGENT_NAME CAS_AGENT_ID CAS_AGENT_ROLE CAS_SESSION_ID CAS_FACTORY_SESSION CAS_FACTORY_MODE CAS_SUPERVISOR_NAME; do
  [[ -z "${!marker:-}" ]] || refuse "factory invocation ($marker); refresh worker guards through factory spawn/reuse"
done
repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"
git_dir="$(git rev-parse --path-format=absolute --git-dir)"
common_dir="$(git rev-parse --path-format=absolute --git-common-dir)"
[[ "$git_dir" == "$common_dir" ]] || refuse "linked worktree; refresh its private guards through factory spawn/reuse"

hooks_dir="$repo_root/.githooks"
# No generated, downloaded, or worker-scoped fallback. Require the reviewed
# source and its existing delegate to be committed and executable in this tree.
for source in .githooks/pre-push scripts/fast-rows-pre-push.sh scripts/release-gate.sh; do
  git ls-files --error-unmatch -- "$source" >/dev/null 2>&1 || refuse "missing tracked source: $source"
  git diff --quiet HEAD -- "$source" || refuse "uncommitted hook source: $source"
  [[ -f "$source" && -x "$source" ]] || refuse "missing executable source: $source"
done

effective="$(git rev-parse --path-format=absolute --git-path hooks)"
# Git only invokes executable hooks. Never overwrite an existing authority or
# hide it with a new hooksPath; the operator must review/back it up separately.
if [[ -d "$effective" ]]; then
  for hook in "$effective"/*; do
    [[ -f "$hook" && -x "$hook" ]] || continue
    [[ "$hook" != *.sample ]] || continue
    if [[ "$hook" == "$hooks_dir/pre-push" ]]; then continue; fi
    refuse "existing hook authority at $hook; inspect and back it up before a reviewed repair"
  done
fi

if [[ "$effective" != "$hooks_dir" ]]; then
  # With worktreeConfig enabled, this changes only the main checkout. Without
  # it, --worktree has Git's documented --local behavior. Worker overrides stay.
  git config --worktree core.hooksPath .githooks
fi
printf 'Main checkout pre-push installed: %s/pre-push\n' "$hooks_dir"
printf 'Factory refs require exact-tip fast rows; other refs pass through.\n'
printf 'Worker private guards require factory spawn/reuse refresh.\n'

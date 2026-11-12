#!/usr/bin/env bash
# Soundwave: run from a trusted checkout as the operator with gh authenticated.
set +x
set -euo pipefail
unset GH_DEBUG
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "$(id -u)" -ne 0 ]]; then
    exec sudo -- "$repo_root/ops/install-light-lanes-5-8.sh" "$(command -v gh)"
fi
: "${SUDO_USER:?Run this script as the operator; it requests sudo once}"
[[ "$SUDO_USER" != root ]] || { echo 'Run as the gh-authenticated operator' >&2; exit 1; }
gh_bin="${1:-$(command -v gh)}"
operator_gh() { runuser -u "$SUDO_USER" -- "$gh_bin" "$@"; }

for slot in {5..8}; do
    token="$(operator_gh api -X POST repos/Richards-LLC/cassy/actions/runners/registration-token -q .token)"
    RUNNER_TOKEN="$token" RUNNER_SLOT="$slot" RUNNER_REGISTRATION=repo \
        "$repo_root/scripts/install-cassy-actions-runner.sh" </dev/null
    unset token
done
# Deploy the idle guard with the new listeners; Rust cache budgets stay unchanged.
install -o root -g root -m 0755 "$repo_root/scripts/prune-cassy-actions-cache.sh" /usr/local/sbin/cassy-actions-cache-prune
install -o root -g root -m 0644 "$repo_root/ops/systemd/cassy-actions-cache-prune.service" /etc/systemd/system/cassy-actions-cache-prune.service
systemctl daemon-reload

for attempt in {1..24}; do
    runners="$(operator_gh api --paginate 'repos/Richards-LLC/cassy/actions/runners?per_page=100' \
        --jq '.runners[] | select(.name | test("^soundwave-cas-lane-[3-8]$")) | [.name, .status, .busy] | @tsv' | sort)"
    if [[ "$(awk '$2 == "online" {online[$1]=1} END {print length(online)}' <<<"$runners")" -eq 6 ]]; then
        printf '%s\n' "$runners"
        exit 0
    fi
    sleep 5
done
printf '%s\n' "$runners" >&2
echo 'Six light runners did not come online within two minutes' >&2
exit 1

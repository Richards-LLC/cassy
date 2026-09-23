#!/usr/bin/env bash
# Self-test for check-scoped-test-surface.sh, including its no-rg fallback.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checker="$script_dir/check-scoped-test-surface.sh"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

repo="$tmpdir/repo"
mkdir -p \
    "$repo/cas-cli/src/builtins/skills/demo" \
    "$repo/cas-cli/tests" \
    "$repo/scripts"
cp "$checker" "$repo/scripts/check-scoped-test-surface.sh"
chmod +x "$repo/scripts/check-scoped-test-surface.sh"

printf '%s\n' 'pub fn factory_widget() {}' >"$repo/cas-cli/src/widget.rs"
printf '%s\n' 'baseline skill' >"$repo/cas-cli/src/builtins/skills/demo/SKILL.md"
printf '%s\n' \
    'use widget::factory_widget;' \
    'fn test_factory_widget() {}' \
    'cas-cli/src/builtins/skills/demo/SKILL.md' \
    'skills/demo/SKILL.md' >"$repo/cas-cli/tests/widget_test.rs"
printf '%s\n' 'builtin flavor contract' >"$repo/cas-cli/tests/builtin_flavor_drift_test.rs"
printf '%s\n' 'agent definition contract' >"$repo/cas-cli/tests/agent_definition_contract_test.rs"
printf '%s\n' 'factory codex guardrail' >"$repo/cas-cli/tests/factory_codex_skill_guardrails.rs"
printf '%s\n' \
    'cas-cli/src/builtins/skills/demo/SKILL.md' \
    'skills/demo/SKILL.md' >"$repo/cas-cli/tests/builtin_demo_test.rs"

git -C "$repo" init -q -b main
git -C "$repo" config user.email scoped-surface@example.test
git -C "$repo" config user.name scoped-surface-test
git -C "$repo" add .
git -C "$repo" commit -qm baseline
git -C "$repo" checkout -qb changed-surface
printf '%s\n' '// changed source surface' >>"$repo/cas-cli/src/widget.rs"
printf '%s\n' 'changed skill' >>"$repo/cas-cli/src/builtins/skills/demo/SKILL.md"
git -C "$repo" add .
git -C "$repo" commit -qm 'change scoped surfaces'

with_rg_output="$(
    cd "$repo"
    bash ./scripts/check-scoped-test-surface.sh --resolve-targets --base main --
)"

# Build an isolated PATH containing every command used by the checker except
# rg. This masks both /usr/bin/rg and /bin/rg on hosts that ship either one.
no_rg_bin="$tmpdir/no-rg-bin"
mkdir -p "$no_rg_bin"
for command in bash git sed basename sort grep; do
    ln -s "$(command -v "$command")" "$no_rg_bin/$command"
done
without_rg_output="$(
    cd "$repo"
    PATH="$no_rg_bin" bash ./scripts/check-scoped-test-surface.sh \
        --resolve-targets --base main --
)"

[[ "$without_rg_output" == "$with_rg_output" ]]
for expected in \
    '--lib widget' \
    '--test widget_test' \
    '--test builtin_demo_test' \
    '--test builtin_flavor_drift_test' \
    '--test agent_definition_contract_test' \
    '--test factory_codex_skill_guardrails'; do
    grep -qF -- "$expected" <<<"$without_rg_output"
done

printf 'ok   no-rg backend preserves scoped target mapping\n'

manifest_repo="$tmpdir/manifest-repo"
mkdir -p "$manifest_repo/cas-cli/tests/hooks_test" "$manifest_repo/scripts"
cp "$checker" "$manifest_repo/scripts/check-scoped-test-surface.sh"
chmod +x "$manifest_repo/scripts/check-scoped-test-surface.sh"
printf '%s\n' \
    '[[test]]' \
    'name = "custom_hooks"' \
    'path = "tests/hooks_test/entry.rs"' >"$manifest_repo/cas-cli/Cargo.toml"
printf '%s\n' '#[test] fn hook_contract() {}' \
    >"$manifest_repo/cas-cli/tests/hooks_test/entry.rs"
git -C "$manifest_repo" init -q -b main
git -C "$manifest_repo" add .
git -C "$manifest_repo" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid commit -qm baseline
git -C "$manifest_repo" checkout -qb changed
printf '%s\n' '#[test] fn hook_contract_changed() {}' \
    >"$manifest_repo/cas-cli/tests/hooks_test/entry.rs"
git -C "$manifest_repo" add .
git -C "$manifest_repo" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid commit -qm 'change explicit test target'
manifest_output="$(cd "$manifest_repo" && bash ./scripts/check-scoped-test-surface.sh --resolve-targets --base main --)"
[[ "$manifest_output" == 'SCOPED_PROOF_TARGET_ARGS: --test custom_hooks' ]]
printf 'ok   manifest test target mapping is preserved\n'

# Two sibling source files can each have an inner `mod tests`. Their proof
# paths must keep the parent module, and a shared prefix filter must cover
# both (the same substring matching used by cargo nextest).
nested_repo="$tmpdir/nested-modules-repo"
mkdir -p "$nested_repo/cas-cli/src/cli" "$nested_repo/cas-cli/tests" "$nested_repo/scripts"
cp "$checker" "$nested_repo/scripts/check-scoped-test-surface.sh"
chmod +x "$nested_repo/scripts/check-scoped-test-surface.sh"
printf '%s\n' 'mod tests { #[test] fn status_case() {} }' \
    >"$nested_repo/cas-cli/src/cli/hub.rs"
printf '%s\n' 'mod tests { #[test] fn authorize_case() {} }' \
    >"$nested_repo/cas-cli/src/cli/hub_reverse_pairing.rs"
git -C "$nested_repo" init -q -b main
git -C "$nested_repo" add .
git -C "$nested_repo" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid commit -qm baseline
git -C "$nested_repo" checkout -qb changed
printf '%s\n' '// status changed' >>"$nested_repo/cas-cli/src/cli/hub.rs"
printf '%s\n' '// authorize changed' >>"$nested_repo/cas-cli/src/cli/hub_reverse_pairing.rs"
git -C "$nested_repo" add .
git -C "$nested_repo" -c user.name=scoped-test-fixture -c user.email=scoped-test-fixture@example.invalid commit -qm 'change sibling modules'
nested_targets="$(cd "$nested_repo" && bash ./scripts/check-scoped-test-surface.sh --resolve-targets --base main --)"
[[ "$nested_targets" == 'SCOPED_PROOF_TARGET_ARGS: --lib cli::hub::tests cli::hub_reverse_pairing::tests' ]]
nested_proof="$(cd "$nested_repo" && bash ./scripts/check-scoped-test-surface.sh --base main -- -p cas --lib cli::hub)"
grep -qF 'SCOPED_PROOF: targets=lib:cli::hub::tests,lib:cli::hub_reverse_pairing::tests result=PASS' <<<"$nested_proof"
if (cd "$nested_repo" && bash ./scripts/check-scoped-test-surface.sh --base main -- -p cas --lib status_case) >/dev/null 2>&1; then
    echo 'FAIL: one status test incorrectly covered both sibling modules' >&2
    exit 1
fi
printf 'ok   sibling nested test modules preserve paths and accept a shared prefix filter\n'
printf 'PASS: scoped test surface backend and nested-module mapping verified.\n'

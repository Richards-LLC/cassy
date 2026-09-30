#!/usr/bin/env bash
# Behavioral usage fixtures for the shipped human-assisted repro template.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
template="$repo_root/cas-cli/src/builtins/skills/cas-diagnosing-bugs/scripts/hitl-loop.template.sh"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
pass=0

bash -n "$template"
pass=$((pass + 1))

printf '\ny\nExample export error\n' |
    APP_URL=http://127.0.0.1:4321 bash "$template" >"$scratch/records" 2>"$scratch/prompts"
diff -u <(printf 'ERRORED=y\nERROR_MSG=Example export error\n') "$scratch/records"
grep -qF 'http://127.0.0.1:4321' "$scratch/prompts"
pass=$((pass + 1))

printf '\nn\nnone\n' | bash "$template" >"$scratch/records" 2>"$scratch/prompts"
diff -u <(printf 'ERRORED=n\nERROR_MSG=none\n') "$scratch/records"
pass=$((pass + 1))

# Quotes, backslashes and shell syntax are observation data, never commands.
observation='literal $(exit 99); "quotes" and \backslashes'
printf '\ny\n%s\n' "$observation" | bash "$template" >"$scratch/records" 2>"$scratch/prompts"
diff -u <(printf 'ERRORED=y\nERROR_MSG=%s\n' "$observation") "$scratch/records"
pass=$((pass + 1))

# EOF at each human interaction must fail without manufacturing a result.
for answers in '' $'\n' $'\ny\n'; do
    if printf '%s' "$answers" | bash "$template" >"$scratch/records" 2>"$scratch/prompts"; then
        printf 'FAIL: incomplete human interaction reported success\n' >&2
        exit 1
    fi
    [[ ! -s "$scratch/records" ]]
    pass=$((pass + 1))
done

printf 'PASS: %s HITL template fixture checks\n' "$pass"

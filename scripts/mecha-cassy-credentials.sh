#!/usr/bin/env bash
# Store the two MechaCassy secrets in this machine's credentials file.
#
# This is the ONLY step in MechaCassy onboarding that a human does by hand:
# `cas integrate mecha-cassy` does everything else, but no agent may ever
# handle a plaintext credential. See docs/MECHA_CASSY_ONBOARDING.md.
#
# Guarantees:
#   - values are read with `read -s` and never echoed, logged, or passed as an
#     argument (so they never appear in `ps` or shell history);
#   - the credentials file is written 0600 and updated in place, never
#     truncating a variable this script does not own;
#   - nothing is sent anywhere. Verification is `cas integrate mecha-cassy`.

set -euo pipefail

CREDENTIALS_FILE="${CAS_CREDENTIALS_FILE:-${XDG_CONFIG_HOME:-$HOME/.config}/cas/credentials.env}"
BYPASS_VAR="MECHA_VERCEL_BYPASS"

die() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

# Replace `export NAME=...` in place, or append it. Reads the value from the
# named variable rather than an argument so it never reaches the process table.
upsert_export() {
    local name="$1" value_var="$2" tmp
    tmp="$(mktemp "${CREDENTIALS_FILE}.XXXXXX")"
    chmod 600 "$tmp"
    if [[ -f "$CREDENTIALS_FILE" ]]; then
        grep -v -E "^[[:space:]]*export[[:space:]]+${name}=" "$CREDENTIALS_FILE" >"$tmp" || true
    fi
    # Single-quote the value and escape any embedded single quote, so a secret
    # containing shell metacharacters is stored literally.
    local escaped=${!value_var//\'/\'\\\'\'}
    printf "export %s='%s'\n" "$name" "$escaped" >>"$tmp"
    mv "$tmp" "$CREDENTIALS_FILE"
    chmod 600 "$CREDENTIALS_FILE"
}

prompt_secret() {
    local prompt="$1" out_var="$2" value=""
    while [[ -z "$value" ]]; do
        printf '%s: ' "$prompt" >&2
        IFS= read -r -s value || die "no input available; run this in an interactive terminal"
        printf '\n' >&2
        [[ -n "$value" ]] || printf 'That was empty. Paste the value from the hub admin.\n' >&2
    done
    printf -v "$out_var" '%s' "$value"
}

[[ -t 0 ]] || die "this wizard needs an interactive terminal (it never takes secrets as arguments)"

printf 'MechaCassy credentials\n'
printf '  file: %s\n\n' "$CREDENTIALS_FILE"

label="${1:-}"
while [[ -z "$label" ]]; do
    printf 'Machine label the hub admin minted a token for (e.g. DANIEL_LAPTOP): ' >&2
    IFS= read -r label || die "no input available"
done
# Match the folding `cas integrate mecha-cassy --label` applies, so the wizard
# and the command always agree on the variable name.
label="$(printf '%s' "$label" | tr '[:lower:]' '[:upper:]' | tr -c '[:alnum:]\n' '_')"
token_var="MECHA_SLACK_TOKEN_${label}"

printf '\nPasting is invisible — nothing is echoed.\n'
prompt_secret "  ${token_var}" token_value
prompt_secret "  ${BYPASS_VAR}" bypass_value

mkdir -p "$(dirname "$CREDENTIALS_FILE")"
chmod 700 "$(dirname "$CREDENTIALS_FILE")" 2>/dev/null || true
: >>"$CREDENTIALS_FILE"
chmod 600 "$CREDENTIALS_FILE"

upsert_export "$token_var" token_value
upsert_export "$BYPASS_VAR" bypass_value
unset token_value bypass_value

profile="$HOME/.bashrc"
[[ -n "${ZSH_VERSION:-}" || "${SHELL:-}" == *zsh ]] && profile="$HOME/.zshrc"
source_line="[ -f \"$CREDENTIALS_FILE\" ] && . \"$CREDENTIALS_FILE\""

printf '\nStored %s and %s (0600, values not shown).\n' "$token_var" "$BYPASS_VAR"
if [[ -f "$profile" ]] && grep -qF "$CREDENTIALS_FILE" "$profile"; then
    printf 'Your %s already sources that file.\n' "$profile"
else
    printf '\nAdd this line to %s so every shell exports them:\n\n  %s\n' "$profile" "$source_line"
fi

printf '\nThen, in a NEW shell:\n\n  cas integrate mecha-cassy --label %s\n  cas doctor\n' "$label"

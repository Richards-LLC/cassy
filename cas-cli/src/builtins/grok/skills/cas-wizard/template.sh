#!/usr/bin/env bash
# Imported and adapted from mattpocock/skills wizard/template.sh, MIT © 2026 Matt Pocock.
# Edit only after STAGES.
set -euo pipefail

TOTAL_STAGES=0
_stage=0
ENV_FILE="${ENV_FILE:-.env}"
open_url() { command -v xdg-open >/dev/null && xdg-open "$1" || command -v open >/dev/null && open "$1" || printf 'Open manually: %s\n' "$1"; }
pause() { read -r -p "${1:-Press Enter to continue} " _ || true; }
confirm() { local answer; read -r -p "$1 [y/N] " answer || true; [[ "$answer" =~ ^[Yy]$ ]]; }
stage() { _stage=$((_stage + 1)); printf '\nStage %s/%s: %s\n' "$_stage" "$TOTAL_STAGES" "$1"; }
say() { printf '  %s\n' "$1"; }
ask() { read -r -p "$2 " "$1" || true; }
ask_secret() { read -r -s -p "$2 " "$1" || true; printf '\n'; }
write_env() { local key="$1" value="$2" tmp; touch "$ENV_FILE"; tmp=$(mktemp); grep -vE "^${key}=" "$ENV_FILE" >"$tmp" || true; printf '%s=%s\n' "$key" "$value" >>"$tmp"; mv "$tmp" "$ENV_FILE"; }
set_secret() { local name="$1" value="$2"; command -v gh >/dev/null && printf '%s' "$value" | gh secret set "$name" || printf 'Set GitHub secret manually: %s\n' "$name"; }
finish() { printf '\nWizard complete.\n'; }

# STAGES: define TOTAL_STAGES and one stage per human action below.
# `confirm` returns 1 on "no", which under `set -e` aborts the whole wizard.
# Always test it with `if`; never call it bare.
# Example:
# TOTAL_STAGES=1
# stage "Dashboard key"; open_url "https://example.invalid"; ask_secret API_KEY "Paste key:"; write_env API_KEY "$API_KEY"
# if confirm "Set it as a GitHub secret too?"; then set_secret API_KEY "$API_KEY"; else say "Skipped."; fi
# finish

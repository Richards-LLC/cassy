#!/usr/bin/env bash
# Adapted from mattpocock/skills (MIT, Copyright 2026 Matt Pocock); see ../LICENSE.
# Copy this file, edit the scenario, and run the copy in the human's terminal.
# APP_URL is a non-secret endpoint; credentials stay in the environment or app.
# `step` waits for an action; `capture` collects one line of redacted observation.
# Capture answers are echoed as KEY=VALUE data, so sign-in belongs in `step`.
# Parse the records as data; never eval them as shell commands.
#
# Usage: APP_URL=http://localhost:3000 bash hitl-loop.template.sh
# Usage fixture (simulated human answers, no app or credentials needed):
#   printf '\ny\nExample export error\n' | bash hitl-loop.template.sh
# Records: ERRORED=y and ERROR_MSG=Example export error

set -euo pipefail

step() {
    local completed
    printf '\n>>> %s\n' "$1" >&2
    if ! read -r -p '    [Enter when done] ' completed; then
        printf 'Incomplete human step; no observation records emitted.\n' >&2
        return 1
    fi
}

capture() {
    local var="$1" question="$2" answer
    [[ "$var" =~ ^[A-Z][A-Z0-9_]*$ ]] || {
        printf 'Invalid observation key: %s\n' "$var" >&2
        return 2
    }
    printf '\n>>> %s\n' "$question" >&2
    if ! read -r -p '    > ' answer; then
        printf 'Missing human observation; no records emitted.\n' >&2
        return 1
    fi
    printf -v "$var" '%s' "$answer"
}

# Edit this scenario to drive the exact symptom. Keep secrets out of captures.
APP_URL="${APP_URL:-http://localhost:3000}"
step "Open the app at $APP_URL and sign in yourself."
capture ERRORED "Click Export. Did it throw an error? (y/n)"
capture ERROR_MSG "Paste the redacted error message (or 'none'):"

printf 'ERRORED=%s\n' "$ERRORED"
printf 'ERROR_MSG=%s\n' "$ERROR_MSG"

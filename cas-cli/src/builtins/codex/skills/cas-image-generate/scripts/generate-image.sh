#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'USAGE'
Usage: generate-image.sh --prompt TEXT --output PATH [--tier draft|final]
                         [--reference PATH]... [--dry-run]

Generates a Google Nano Banana image. The credential is read from
GEMINI_API_KEY; use --dry-run to validate routing without an API request.
USAGE
}

prompt=""
output=""
tier="draft"
dry_run=false
references=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prompt)
            [[ $# -ge 2 ]] || { echo "error: --prompt needs a value" >&2; usage; exit 2; }
            prompt="$2"
            shift 2
            ;;
        --output)
            [[ $# -ge 2 ]] || { echo "error: --output needs a value" >&2; usage; exit 2; }
            output="$2"
            shift 2
            ;;
        --tier)
            [[ $# -ge 2 ]] || { echo "error: --tier needs a value" >&2; usage; exit 2; }
            tier="$2"
            shift 2
            ;;
        --reference)
            [[ $# -ge 2 ]] || { echo "error: --reference needs a value" >&2; usage; exit 2; }
            references+=("$2")
            shift 2
            ;;
        --dry-run)
            dry_run=true
            shift
            ;;
        -h|--help)
            usage >&1
            exit 0
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            usage
            exit 2
            ;;
    esac
done

if [[ -z "$prompt" || -z "$output" ]]; then
    echo "error: --prompt and --output are required" >&2
    usage
    exit 2
fi

case "$tier" in
    draft) model="gemini-3.1-flash-image" ;;
    final) model="gemini-3-pro-image" ;;
    *)
        echo "error: --tier must be draft or final" >&2
        exit 2
        ;;
esac

if [[ -z "${GEMINI_API_KEY:-}" ]]; then
    cat >&2 <<'MISSING_KEY'
error: GEMINI_API_KEY is not set; Nano Banana generation is unavailable.
Create a Google AI Studio API key and export GEMINI_API_KEY in the process
environment (or load it through your local secret manager), then retry.
The helper does not read or store keys from project files.
MISSING_KEY
    exit 2
fi

if [[ "$dry_run" == true ]]; then
    printf 'provider=google-nano-banana\nmodel=%s\ntier=%s\nreferences=%d\ndry_run=true\n' \
        "$model" "$tier" "${#references[@]}"
    exit 0
fi

for reference in "${references[@]}"; do
    if [[ ! -f "$reference" ]]; then
        echo "error: reference file does not exist: $reference" >&2
        exit 2
    fi
done

mime_type() {
    case "${1,,}" in
        *.png) printf 'image/png' ;;
        *.webp) printf 'image/webp' ;;
        *.gif) printf 'image/gif' ;;
        *.jpg|*.jpeg) printf 'image/jpeg' ;;
        *)
            echo "error: unsupported reference type (use png, jpeg, webp, or gif): $1" >&2
            exit 2
            ;;
    esac
}

parts="$(jq -n --arg prompt "$prompt" '[{text: $prompt}]')"
for reference in "${references[@]}"; do
    encoded="$(base64 --wrap=0 "$reference")"
    reference_mime="$(mime_type "$reference")"
    parts="$(jq --arg mime "$reference_mime" --arg data "$encoded" \
        '. + [{inlineData: {mimeType: $mime, data: $data}}]' <<<"$parts")"
done

payload="$(jq -n --argjson parts "$parts" '{contents: [{parts: $parts}]}')"
endpoint="https://generativelanguage.googleapis.com/v1beta/models/${model}:generateContent"
response=""
if ! response="$(curl -sS --connect-timeout 15 --max-time 180 \
    -H "x-goog-api-key: $GEMINI_API_KEY" \
    -H 'Content-Type: application/json' \
    -d "$payload" "$endpoint")"; then
    echo "error: Nano Banana request failed; check network access and Google AI Studio credentials" >&2
    exit 1
fi

image_data="$(jq -r '
    first(.candidates[]?.content.parts[]? | (.inlineData // .inline_data) | .data) // empty
' <<<"$response")"
if [[ -z "$image_data" ]]; then
    api_error="$(jq -r '.error.message // empty' <<<"$response" 2>/dev/null || true)"
    if [[ -n "$api_error" ]]; then
        echo "error: Nano Banana returned an API error: $api_error" >&2
    else
        echo "error: Nano Banana response contained no image data" >&2
    fi
    exit 1
fi

mkdir -p "$(dirname "$output")"
printf '%s' "$image_data" | base64 --decode > "$output"
printf 'wrote=%s\nmodel=%s\n' "$output" "$model"

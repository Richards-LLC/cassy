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

reference_mime_type() {
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

output_extension_for_mime() {
    case "${1,,}" in
        image/png) printf 'png' ;;
        image/jpeg|image/jpg) printf 'jpg' ;;
        image/webp) printf 'webp' ;;
        image/gif) printf 'gif' ;;
        *) return 1 ;;
    esac
}

extension_matches_mime() {
    case "${1,,}:${2,,}" in
        image/png:png|image/jpeg:jpg|image/jpeg:jpeg|image/jpg:jpg|image/jpg:jpeg|image/webp:webp|image/gif:gif)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

printf '%s' "$prompt" > "$work/prompt.txt"
jq -n --rawfile prompt "$work/prompt.txt" '[{text: $prompt}]' > "$work/parts.json"
reference_index=0
for reference in "${references[@]}"; do
    base64 --wrap=0 "$reference" | tr -d '\r\n' > "$work/reference-$reference_index.b64"
    reference_mime="$(reference_mime_type "$reference")"
    jq --arg mime "$reference_mime" --rawfile data "$work/reference-$reference_index.b64" \
        '. + [{inlineData: {mimeType: $mime, data: $data}}]' \
        "$work/parts.json" > "$work/parts.next.json"
    mv "$work/parts.next.json" "$work/parts.json"
    reference_index=$((reference_index + 1))
done

jq '{contents: [{parts: .}]}' "$work/parts.json" > "$work/payload.json"
endpoint="https://generativelanguage.googleapis.com/v1beta/models/${model}:generateContent"
if ! curl -sS --connect-timeout 15 --max-time 180 \
    -H "x-goog-api-key: $GEMINI_API_KEY" \
    -H 'Content-Type: application/json' \
    --data-binary "@$work/payload.json" "$endpoint" > "$work/response.json"; then
    echo "error: Nano Banana request failed; check network access and Google AI Studio credentials" >&2
    exit 1
fi

image_data="$(jq -r '
    first(.candidates[]?.content.parts[]? | (.inlineData // .inline_data) | .data) // empty
' "$work/response.json")"
if [[ -z "$image_data" ]]; then
    api_error="$(jq -r '.error.message // empty' "$work/response.json" 2>/dev/null || true)"
    if [[ -n "$api_error" ]]; then
        echo "error: Nano Banana returned an API error: $api_error" >&2
    else
        echo "error: Nano Banana response contained no image data" >&2
    fi
    exit 1
fi

returned_mime="$(jq -r '
    first(.candidates[]?.content.parts[]? | (.inlineData // .inline_data) |
        (.mimeType // .mime_type)) // empty
' "$work/response.json")"
final_output="$output"
if [[ -n "$returned_mime" ]]; then
    returned_mime="${returned_mime%%;*}"
    if actual_extension="$(output_extension_for_mime "$returned_mime")"; then
        output_name="$(basename "$output")"
        requested_extension=""
        if [[ "$output_name" == *.* ]]; then
            requested_extension="${output_name##*.}"
        fi
        if ! extension_matches_mime "$returned_mime" "$requested_extension"; then
            output_stem="${output_name%.*}"
            if [[ "$output_name" != *.* ]]; then
                output_stem="$output_name"
            fi
            output_dir="$(dirname "$output")"
            if [[ "$output_dir" == "." ]]; then
                final_output="$output_stem.$actual_extension"
            else
                final_output="$output_dir/$output_stem.$actual_extension"
            fi
            echo "warning: MIME mismatch: API returned $returned_mime but requested output '$output'; writing '$final_output'" >&2
        fi
    else
        echo "warning: API returned unsupported image MIME '$returned_mime'; writing requested output '$output'" >&2
    fi
else
    echo "warning: API response omitted image MIME type; writing requested output '$output'" >&2
fi

mkdir -p "$(dirname "$final_output")"
printf '%s' "$image_data" | base64 --decode > "$final_output"
printf 'wrote=%s\nmodel=%s\n' "$final_output" "$model"

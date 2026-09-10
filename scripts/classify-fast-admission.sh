#!/usr/bin/env bash
# Classify whether a committed diff may use the fast scoped admission path.
#
# This is intentionally shared by release-train and CI.  A fast admission is
# an optimization for a small, ordinary source delta; manifest/lock changes
# and generated Commander assets stay on the full validation path.
set -euo pipefail

usage() {
    printf 'usage: %s <base-ref> <head-ref>\n' "$0" >&2
    exit 2
}

[[ $# -eq 2 ]] || usage
base_ref="$1"
head_ref="$2"
max_files="${CAS_FAST_ADMISSION_MAX_FILES:-${CAS_RELEASE_TRAIN_FAST_MAX_FILES:-5}}"
if [[ ! "$max_files" =~ ^[0-9]+$ ]]; then
    printf 'fast admission: invalid max file count %q (expected a non-negative integer)\n' \
        "$max_files" >&2
    exit 2
fi

if ! diff_list="$(git diff --name-only "$base_ref" "$head_ref")"; then
    printf 'fast admission: cannot inspect diff %s..%s; refusing admission\n' \
        "$base_ref" "$head_ref" >&2
    exit 1
fi

display_ref() {
    local ref="$1" short
    short="$(git rev-parse --short "${ref}^{commit}" 2>/dev/null || true)"
    printf '%s\n' "${short:-$ref}"
}

files=()
while IFS= read -r path; do
    [[ -n "$path" ]] || continue
    files[${#files[@]}]="$path"
done <<<"$diff_list"

reasons=()
for path in "${files[@]}"; do
    case "$path" in
        Cargo.toml|*/Cargo.toml)
            reasons[${#reasons[@]}]='manifest'
            ;;
        Cargo.lock|*/Cargo.lock)
            reasons[${#reasons[@]}]='lock'
            ;;
        hub-web/dist|hub-web/dist/*)
            reasons[${#reasons[@]}]='generated-assets'
            ;;
    esac
done

eligible=true
if (( ${#files[@]} > max_files )); then
    eligible=false
    reasons[${#reasons[@]}]="file-count:${#files[@]}>${max_files}"
fi
if (( ${#reasons[@]} > 0 )); then
    eligible=false
fi

reason=none
if (( ${#reasons[@]} > 0 )); then
    reason="$(IFS=,; printf '%s' "${reasons[*]}")"
fi

printf 'FAST_ADMISSION: eligible=%s files=%s max_files=%s reason=%s\n' \
    "$eligible" "${#files[@]}" "$max_files" "$reason"
printf 'comparison: base=%s head=%s\n' "$(display_ref "$base_ref")" "$(display_ref "$head_ref")"
if [[ "$eligible" != true ]]; then
    exit 1
fi

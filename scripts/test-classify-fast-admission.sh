#!/usr/bin/env bash
# Fixture tests for the small-delta admission classifier (cas-2fcf).
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
classifier="$script_dir/classify-fast-admission.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

repo="$tmp/repo"
mkdir -p "$repo"
git -C "$repo" init -q -b main
git -C "$repo" config user.email test@test.invalid
git -C "$repo" config user.name 'Fast Admission Test'
printf 'base\n' >"$repo/README.md"
git -C "$repo" add README.md
git -C "$repo" -c commit.gpgsign=false commit -qm base
base="$(git -C "$repo" rev-parse HEAD)"

check() {
    local expected="$1" label="$2" output status
    shift 2
    set +e
    output="$(cd "$repo" && "$classifier" "$@" 2>&1)"
    status=$?
    set -e
    if [[ "$expected" == true && "$status" -eq 0 && "$output" == *'eligible=true'* ]]; then
        printf 'ok   %s\n' "$label"
        return 0
    fi
    if [[ "$expected" == false && "$status" -ne 0 && "$output" == *'eligible=false'* ]]; then
        printf 'ok   %s\n' "$label"
        return 0
    fi
    printf 'FAIL %s (status=%s output=%s)\n' "$label" "$status" "$output"
    return 1
}

printf 'one\n' >"$repo/one.txt"
git -C "$repo" add one.txt
git -C "$repo" -c commit.gpgsign=false commit -qm one
one="$(git -C "$repo" rev-parse HEAD)"
check true 'one ordinary file is fast-admissible' "$base" "$one"

for file in Cargo.toml Cargo.lock hub-web/dist/bundle.js; do
    git -C "$repo" checkout -q "$one"
    mkdir -p "$(dirname "$repo/$file")"
    printf 'excluded\n' >"$repo/$file"
    git -C "$repo" add "$file"
    git -C "$repo" -c commit.gpgsign=false commit -qm "excluded $file"
    head="$(git -C "$repo" rev-parse HEAD)"
    check false "$file is not fast-admissible" "$one" "$head"
done

git -C "$repo" checkout -q "$one"
for number in 1 2 3 4 5 6; do
    printf '%s\n' "$number" >"$repo/file-$number.txt"
    git -C "$repo" add "file-$number.txt"
done
git -C "$repo" -c commit.gpgsign=false commit -qm 'six-file delta'
six="$(git -C "$repo" rev-parse HEAD)"
check false 'six files exceed the default five-file limit' "$one" "$six"
configured="$(cd "$repo" && CAS_FAST_ADMISSION_MAX_FILES=6 "$classifier" "$one" "$six")"
if [[ "$configured" == *'eligible=true'* ]]; then
    printf 'ok   configurable limit admits the same six files\n'
else
    printf 'FAIL configurable limit rejected six files: %s\n' "$configured"
    exit 1
fi

set +e
invalid="$(cd "$repo" && CAS_FAST_ADMISSION_MAX_FILES=oops "$classifier" "$one" "$six" 2>&1)"
invalid_status=$?
set -e
if [[ "$invalid_status" -eq 2 && "$invalid" == *'invalid max file count'* ]]; then
    printf 'ok   invalid size configuration fails closed\n'
else
    printf 'FAIL invalid size configuration was accepted (status=%s output=%s)\n' \
        "$invalid_status" "$invalid"
    exit 1
fi

printf 'test result: fast admission classifier passed\n'

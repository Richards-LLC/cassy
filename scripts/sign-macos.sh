#!/usr/bin/env bash
# cas-fe62: sign the final (already stripped) bytes; never sign after packaging.
# prebuild: optional secrets, Developer ID only when configured.
# candidate: login keychain, Developer ID first, Apple Development second.
# verify-release: mandatory secrets + Developer ID signature on the adopted bytes.
set -euo pipefail
set +x
mode="${1:-}"
binary="${2:-}"
case "$mode" in prebuild|candidate|verify-release) ;; *) echo 'usage: sign-macos.sh prebuild|candidate|verify-release <cas-binary>' >&2; exit 2 ;; esac
[[ -f "$binary" ]] || { echo "error: binary missing: $binary" >&2; exit 1; }
started=$SECONDS
signing_tmp=''
cleanup() {
    if [[ -n "$signing_tmp" ]]; then
        security delete-keychain "$signing_tmp/signing.keychain-db" >/dev/null 2>&1 || true
        rm -rf "$signing_tmp"
    fi
    local elapsed=$((SECONDS - started))
    printf 'macOS signing (%s): %s seconds\n' "$mode" "$elapsed"
    if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
        printf 'macOS signing (%s): %s seconds\n' "$mode" "$elapsed" >> "$GITHUB_STEP_SUMMARY"
    fi
}
trap cleanup EXIT
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }
secrets_complete() {
    [[ -n "${MACOS_SIGNING_CERT_P12_BASE64:-}" && -n "${MACOS_SIGNING_CERT_PASSWORD:-}" && -n "${MACOS_SIGNING_TEAM_ID:-}" ]]
}
require_secrets() {
    secrets_complete || fail 'publishing requires MACOS_SIGNING_CERT_P12_BASE64, MACOS_SIGNING_CERT_PASSWORD and MACOS_SIGNING_TEAM_ID; run the cas-fe62 signing wizard on prowl'
    [[ "$MACOS_SIGNING_TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || fail 'MACOS_SIGNING_TEAM_ID must be the 10-character Apple team ID'
}
# Developer ID labels contain the team; Apple Development labels may instead
# contain the person's ID. Always read the actual team from the signed bytes.
choose_identity() {
    local kind="$1" row team
    while IFS= read -r row; do
        if [[ "$row" =~ ([A-Fa-f0-9]{40})\ \"$kind:\ .+\ \(([A-Z0-9]{10})\)\" ]]; then
            team="${BASH_REMATCH[2]}"
            if [[ "$kind" == 'Apple Development' || -z "${MACOS_SIGNING_TEAM_ID:-}" || "$team" == "$MACOS_SIGNING_TEAM_ID" ]]; then
                identity="${BASH_REMATCH[1]}"
                identity_team="$team"
                return 0
            fi
        fi
    done <<< "$identities"
    return 1
}
verify_identity() {
    local expected_team="$1" release="$2" detail
    codesign --verify --strict --verbose=4 "$binary"
    detail="$(codesign -dv --verbose=4 "$binary" 2>&1)"
    printf '%s\n' "$detail"
    grep -Fxq 'Identifier=dev.petrastella.cas' <<< "$detail" || fail 'signing identifier does not match dev.petrastella.cas'
    identity_team="$(sed -n 's/^TeamIdentifier=//p' <<< "$detail")"
    [[ "$identity_team" =~ ^[A-Z0-9]{10}$ ]] || fail 'signature has no valid Apple TeamIdentifier'
    [[ -n "$expected_team" ]] || expected_team="$identity_team"
    [[ "$identity_team" == "$expected_team" ]] || fail 'signature TeamIdentifier does not match the selected Apple team'
    if [[ "$release" == true ]]; then
        grep -q '^Authority=Developer ID Application: ' <<< "$detail" || fail 'release requires Developer ID Application; development/ad-hoc signatures are not publishable'
    fi
    # Verify the certificate anchor and team, not just the displayed strings.
    codesign --verify --strict -R "=anchor apple generic and identifier \"dev.petrastella.cas\" and certificate leaf[subject.OU] = \"$expected_team\"" "$binary"
}
if [[ "$mode" == verify-release ]]; then
    require_secrets
    verify_identity "$MACOS_SIGNING_TEAM_ID" true
    # Internal distribution is deliberately not notarized. Gatekeeper assessment
    # is diagnostic; signature/identity verification above is the publication gate.
    if ! spctl --assess --type execute --verbose=4 "$binary"; then
        echo 'Gatekeeper assessment rejected (notarization is optional); not a publication gate.'
    fi
    exit 0
fi
keychain_args=()
if [[ "$mode" == prebuild ]]; then
    if ! secrets_complete; then
        # Even partial configuration is safe on a queue/prebuild without secrets.
        echo '::warning::unsigned candidate, not publishable: signing secrets are absent or incomplete'
        codesign --force --identifier dev.petrastella.cas --sign - "$binary"
        codesign --verify --strict --verbose=4 "$binary"
        exit 0
    fi
    require_secrets
    umask 077
    signing_tmp="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/cas-signing.XXXXXX")"
    printf '%s' "$MACOS_SIGNING_CERT_P12_BASE64" | base64 --decode > "$signing_tmp/certificate.p12"
    keychain_password="$(openssl rand -hex 24)"
    keychain="$signing_tmp/signing.keychain-db"
    security create-keychain -p "$keychain_password" "$keychain"
    security set-keychain-settings -lut 21600 "$keychain"
    security unlock-keychain -p "$keychain_password" "$keychain"
    security import "$signing_tmp/certificate.p12" -P "$MACOS_SIGNING_CERT_PASSWORD" -t cert -f pkcs12 -k "$keychain" -T /usr/bin/codesign
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
    keychain_args=(--keychain "$keychain")
    identities="$(security find-identity -v -p codesigning "$keychain")"
    choose_identity 'Developer ID Application' || fail 'no valid Developer ID Application identity for MACOS_SIGNING_TEAM_ID in the imported certificate'
else
    # Never search unrelated system keychains or import/export an operator key.
    keychain="${HOME}/Library/Keychains/login.keychain-db"
    keychain_args=(--keychain "$keychain")
    identities="$(security find-identity -v -p codesigning "$keychain" 2>/dev/null)" || identities=''
    if ! choose_identity 'Developer ID Application' && ! choose_identity 'Apple Development'; then
        echo '::warning::unsigned candidate, not publishable: no matching login-keychain signing identity; using ad-hoc signing'
        codesign --force --identifier dev.petrastella.cas --sign - "$binary"
        codesign --verify --strict --verbose=4 "$binary"
        exit 0
    fi
fi
codesign --force --options runtime --timestamp --identifier dev.petrastella.cas \
    --sign "$identity" "${keychain_args[@]}" "$binary"
verify_identity "${MACOS_SIGNING_TEAM_ID:-}" "$([[ "$mode" == prebuild ]] && echo true || echo false)"
printf 'Signed %s; TeamIdentifier=%s\n' "$binary" "$identity_team"

# Slack draft — CAS vX.Y.Z runtime release

Channel: #cas-internal (C0B44GUKDK2)

**Status:** Draft. Do not post until the tagged GitHub workflow publishes both
assets and `release-published-receipt.sh --write-draft` has replaced both
checksum placeholders below.

## User thread

**Top-level:**
Live on production — **User** — {{USER_IMPACT_PUNCH}}

**Reply (Was → Now):**
- Was: {{USER_WAS}} Now: {{USER_NOW}}
- Install: use `cas update` for an existing installation, or download the
  vX.Y.Z archive for Linux x86_64 (SHA-256 `{{LINUX_SHA256}}`) or macOS ARM64
  (SHA-256 `{{MACOS_SHA256}}`) from the GitHub release.

## Dev thread

**Top-level:**
Live on production — **Dev** — {{DEV_IMPACT_PUNCH}}

**Reply (Was → Now):**
- Was: {{DEV_WAS}} Now: {{DEV_NOW}}
- Validation and artifact: {{VALIDATION_SUMMARY}} The published archives are
  `cas-x86_64-unknown-linux-gnu.tar.gz` (SHA-256 `{{LINUX_SHA256}}`) and
  `cas-aarch64-apple-darwin.tar.gz` (SHA-256 `{{MACOS_SHA256}}`). Linux and
  macOS are both available from CI, regardless of the tagger's host.

## Posting sequence

1. Replace the narrative placeholders after the release PR is merged.
2. Tag the fetched `origin/main` landing and run `./scripts/release.sh`.
3. Run `release-published-receipt.sh --write-draft` against this draft. It
   downloads and hashes both published assets before replacing every digest
   placeholder; never type a digest from a local audit archive.
4. Post the User top-level and its only reply, then the Dev top-level and its
   only reply; append the receipt below.

## POSTED

Channel: `#cas-internal` (`C0B44GUKDK2`).

| Message | UTC timestamp | Permalink |
| --- | --- | --- |
| User top-level |  |  |
| User reply (Was → Now) |  |  |
| Dev top-level |  |  |
| Dev reply (Was → Now) |  |  |

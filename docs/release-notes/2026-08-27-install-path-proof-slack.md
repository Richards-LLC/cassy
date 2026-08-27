# 2026-08-27 — Install path proof on real machines — #cas-internal

> EMBARGO: do not post before 2026-08-31 (operator confirmation required).
> Draft complete; append the POSTED receipt table after publication.

## User thread

**Top-level (Live on production · User):**

🧪 Every Cassy release now proves its one-line installer on a real Mac and a
clean Linux box before anyone may claim "installation works."

**Reply (Was → Now):**

Was: the install instructions were checked by script unit tests and doc review —
nobody executed the full new-user journey on a real machine before a release
went out, so "it installs" was a promise, not a fact.

Now: publishing a release automatically runs the actual `curl … | sh` install on
a hosted Apple Silicon Mac and a bare Ubuntu container with nothing preinstalled.
The check opens a brand-new terminal and confirms `cas --version`, plain `cas`,
and `cas doctor` all work, and it keeps the full session transcripts as
downloadable proof. A release may only say "install works" with that green
receipt attached. The one thing a hosted Mac cannot show — the graphical
Gatekeeper prompt on a consumer machine — stays on a written manual checklist
instead of being quietly claimed as covered.

## Dev thread

**Top-level (Live on production · Dev):**

⚙️ New advisory `Install path proof` workflow runs the published
`cas-install.sh` end-to-end on `macos-latest` (arm64) and a clean
`ubuntu:24.04` container on every `release.published`, uploading transcript
artifacts as the receipt (PR #591).

**Reply (Was → Now):**

Was: install-path confidence came from `scripts/test-cas-install.sh` fixtures
and doc review; no CI job executed the released installer against published
assets, and release copy could claim installability with no runtime evidence.

Now: `.github/workflows/install-path-proof.yml` triggers on `release.published`
(so `CAS_VERSION` always names downloadable assets) and via
`workflow_dispatch -f version=vX.Y.Z`. The macOS job asserts Darwin/arm64, runs
the real installer into a clean `HOME`, verifies a fresh zsh login shell finds
`cas` on PATH, checks no `com.apple.quarantine` attribute survives without
manual `xattr`, and greps the plain-language success contracts; the Linux job
does the same in a container that provably lacks `rustc`. Both upload 90-day
transcript artifacts. The lane is advisory (not branch-protection-required)
until explicitly promoted; the release checklist now demands the green run URL
plus both transcripts before any installability claim, with the GUI
Gatekeeper/SIP surface documented as a manual consumer-Mac checklist item.

## POSTED

(to be filled at publication — parent/reply permalinks + timestamps for both threads)

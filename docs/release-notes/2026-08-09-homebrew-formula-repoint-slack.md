# Slack release notes — 2026-08-09 — Homebrew formula repoint

Channel: `#cas-internal` (`C0B44GUKDK2`)
Merge: `main` @ `5e10edcd` — Homebrew formula now installs the current release from the active repository.

---

## User thread

**Top-level:**

Live on production — **User**: `brew install` now delivers the current Cassy release on Apple Silicon Macs and Linux instead of a months-old version from a dead download link.

**Reply:**

Was: the Homebrew formula pointed at an abandoned repository, pinned to version 0.2.1 — dozens of releases behind, with download links that no longer matched anything actually published. Now: it installs v2.55.5 directly from the project's real releases, with checksums verified against the actual published files. On platforms without a build (Intel macOS, ARM Linux) it now tells you clearly instead of failing on a broken download. Install with `brew install --formula ./homebrew/cas.rb`.

---

## Dev thread

**Top-level:**

Live on production — **Dev**: `homebrew/cas.rb` repointed from `codingagentsystem/cas` v0.2.1 to `pippenz/cas` v2.55.5 with freshly computed SHA-256s.

**Reply:**

Was: formula URLs targeted `github.com/codingagentsystem/cas` at v0.2.1 with stale sha256 values, including a macOS arm64 asset that pipeline never produced. Now: URLs target `github.com/pippenz/cas/releases/download/v2.55.5/`; both published artifacts (`cas-aarch64-apple-darwin.tar.gz`, `cas-x86_64-unknown-linux-gnu.tar.gz`) carry recomputed SHA-256s independently verified against fresh downloads, and the unsupported `on_intel` (macOS) / `on_arm` (Linux) branches `odie` with an explicit message instead of 404ing at install time.

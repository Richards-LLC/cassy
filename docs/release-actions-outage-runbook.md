# Release Actions-Outage Runbook

Use this only when [GitHub Status](https://www.githubstatus.com/) reports **Actions** degraded/unavailable, or fresh release runs cannot acquire a runner. The normal tag-triggered workflow remains the authoritative publisher when Actions is healthy.

## First decide: wait, retry, or fail over

1. Check GitHub Status and the queued timestamp. A queued run is an outage signal, not evidence that the release source is bad.
2. Queue **one fresh rerun** after recovery is indicated. Do not repeatedly rerun an outage-era job: during the 2026-08-17 incident, those runs stayed wedged while newly-created runs progressed.
3. Do not foreground-watch `gh run watch` or poll in a loop. In a factory session, record the run URL/ID, set a CAS reminder, end the turn, and check once when it fires. A 10–15 minute reminder is appropriate while GitHub Status remains degraded.
4. If Actions still cannot publish and the release owner accepts the missing CI evidence, build the Linux asset below. The GitHub REST/Release API can remain available while Actions runners are not.

Git-over-SSH may also remain healthy while GitHub’s merge API returns 5xx. It does not bypass protected-main checks: keep the release-source PR open and merge it through the normal protected-branch path after recovery.

## Linux x86_64 fallback artifact

Run these commands from a clean checkout of the exact annotated release tag on a Linux x86_64 host. They mirror the Linux build job in `.github/workflows/release.yml`: Zig 0.15.2 from `bootstrap-zig.sh`, the same `--release --target x86_64-unknown-linux-gnu --locked` Cargo invocation, and the same portability audits.

```bash
TAG="$(git describe --exact-match --tags HEAD)"
test "$TAG" = "v$(sed -n 's/^version = "\([^"]*\)"/\1/p' cas-cli/Cargo.toml | head -1)"
./scripts/check-release-preflight.sh --local "$TAG"

./scripts/bootstrap-zig.sh
export ZIG="$PWD/.context/zig/zig"
export PATH="$PWD/.context/zig:$PATH"
rustup target add x86_64-unknown-linux-gnu

cargo clean -p blake3 --release --target x86_64-unknown-linux-gnu
cargo build -p cas --release --target x86_64-unknown-linux-gnu --locked

OUT="$PWD/dist/actions-outage/$TAG"
STAGE="$(mktemp -d)"
mkdir -p "$OUT"
cp target/x86_64-unknown-linux-gnu/release/cas LICENSE "$STAGE/"
./scripts/check-blake3-no-avx512-build.sh target/x86_64-unknown-linux-gnu/release/build
./scripts/test-check-portable-x86_64-isa.sh "$STAGE/cas"
tar -C "$STAGE" -czvf "$OUT/cas-x86_64-unknown-linux-gnu.tar.gz" cas LICENSE
rm -rf "$STAGE"
sha256sum "$OUT/cas-x86_64-unknown-linux-gnu.tar.gz"
```

`scripts/release.sh` remains the guarded helper for the full local-audit and manual-publish flow. Its `--manual-publish` mode deliberately requires both `--publish-tag` and `--acknowledge-workflow-conflict`; do not weaken those guards for an outage.

## macOS ARM64 is not optional

A Linux host cannot produce the native `aarch64-apple-darwin` release artifact. Do **not** create a Linux-only public release: `release-published-receipt.sh` requires both named assets before an announcement can be made.

Choose one path before publishing:

- **Wait for Actions recovery** — preferred for a short outage. Let the normal workflow build both assets and publish.
- **Use a Mac checkout** — on a macOS ARM64 host, check out the same annotated tag and build/package the second asset:

  ```bash
  export DEVELOPER_DIR=/Applications/Xcode_26.3.app/Contents/Developer
  ./scripts/bootstrap-zig.sh
  export ZIG="$PWD/.context/zig/zig"
  cargo build -p cas --release --target aarch64-apple-darwin --locked
  STAGE="$(mktemp -d)"
  cp target/aarch64-apple-darwin/release/cas LICENSE "$STAGE/"
  tar -C "$STAGE" -czvf "cas-aarch64-apple-darwin.tar.gz" cas LICENSE
  rm -rf "$STAGE"
  ```

Copy that archive to the Linux release checkout (or another trusted release host) before creating the release.

## Manual publication after both assets exist

First verify no release object already exists. Existing or partial releases must follow [the recovery procedure](RELEASE_SLACK_RUBRIC.md#recovering-a-failed-or-partial-release); never replace assets in place.

```bash
REPO=Richards-LLC/cassy
LINUX_ASSET="$PWD/dist/actions-outage/$TAG/cas-x86_64-unknown-linux-gnu.tar.gz"
MACOS_ASSET="$PWD/cas-aarch64-apple-darwin.tar.gz"
test -s "$LINUX_ASSET" && test -s "$MACOS_ASSET"
gh release view "$TAG" --repo "$REPO" && exit 1
```

Dry-run the exact publish command without changing GitHub:

```bash
printf 'gh release create %q --repo %q --title %q --generate-notes %q %q\n' \
  "$TAG" "$REPO" "CAS $TAG" "$LINUX_ASSET" "$MACOS_ASSET"
```

After the release owner approves that output, run the printed command. Then run `./scripts/release-published-receipt.sh "$TAG" --write-draft <draft>` before any release announcement. The published GitHub assets—not a local archive or its checksum—are the source for announcement digests.

## Standing self-hosted Linux runner: not a commitment

A self-hosted Linux runner could reduce runner-queue exposure, but it does not solve the required macOS ARM64 build. It also adds patching, runner-token, isolation, and secret-handling obligations. Given that the manual Linux lane above is sufficient for a rare outage and publication still needs a Mac or Actions recovery, do not adopt one without an owner, budget, and security review.

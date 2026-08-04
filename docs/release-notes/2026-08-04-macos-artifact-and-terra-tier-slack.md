# 2026-08-04 — v2.40.0 macOS release artifact + terra default tier — #cas-internal posts

## Post 1 — User

**Live on production — User** (v2.40.0)

Was: on a Mac, updating meant building from source — the published release only ever contained a Linux binary, so there was nothing to download. Now: releases carry a macOS build too.

- If you are on Apple silicon and have been building locally to get each update, you no longer have to. The download you expected has been missing from every release, not just the recent ones.
- The release will now fail outright rather than publish with a platform missing. That is deliberate: a release that quietly ships half of what it should is worse than one that stops and tells you.
- Separately, new workers now start on a different default model tier, with the previous default reserved for the heaviest work. If you have been passing model settings explicitly you will see no change; if you relied on the default, the work gets a different tier than before.

## Post 2 — Dev

**Live on production — Dev** (v2.40.0)

Was: the release workflow defined a single build job targeting `x86_64-unknown-linux-gnu`, while the local release script targets both platforms and the Homebrew formula requests `cas-aarch64-apple-darwin.tar.gz`. The automated path produced neither the artifact the formula needs nor a Mac download. Now: a macOS job builds and packages it, and the publish step waits on both.

- The macOS job reuses the configuration that took several CI cycles to establish: the runner is pinned, and Xcode is selected explicitly through `DEVELOPER_DIR`. Zig 0.15.2 cannot link against the newer SDK the rolling macOS image now selects, and it discovers Darwin SDKs through `xcrun` — which follows `DEVELOPER_DIR`, **not** `SDKROOT`. The reason is recorded in a comment above the pin, because a pin without a rationale gets removed by the next person tidying up.
- The publish step already downloaded all artifacts and globbed for tarballs, so it collects the new archive with no change. It now depends on both builds. A macOS failure therefore blocks the entire release rather than publishing a Linux-only one — that is the intended trade, and no `continue-on-error` was added to soften it.
- Verified before shipping by building the exact target natively: `cargo build -p cas --release --target aarch64-apple-darwin` exits 0, and packaging produces precisely the expected archive. That is the first evidence this workspace **links** under the release profile for Darwin; the existing macOS CI job only ever ran `cargo check`, so linking had never been exercised for that target in any form.
- Honest boundary: the runner environment, cache actions, artifact upload and asset attachment cannot be exercised without cutting a tag. This release is that test.
- Also in this release: the default worker tier moves to `gpt-5.6-terra` at high effort, with the previous default reserved for heavy and frontier work. Supervisor guidance, the model-selection reference and the code-review workflow were retiered together, and current model slugs are now documented rather than passed around as folklore.
- Both changes landed from independent sessions working the same repository within an hour of each other, including edits to the same integration-test file. The combined state was verified before tagging: 3,160 library tests and the full factory integration target pass. Two separately-green branches do not prove a green merge.

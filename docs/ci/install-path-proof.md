# Install path proof

The install proof is the executable release check for the user journey behind
the one-line installer. It is advisory: a green run is required before release
copy claims that installation works, but it is not a required branch-protection
check until the operator explicitly promotes it.

## When it runs

`.github/workflows/install-path-proof.yml` runs after every GitHub Release is
published (`release.published`). Waiting for publication matters: a tag push can
start before the release publisher has uploaded the two archives, while this
event guarantees that `CAS_VERSION` points at downloadable release assets.

It is also dispatchable for a historical or current release:

```bash
gh workflow run install-path-proof.yml --repo Richards-LLC/cassy -f version=v3.4.1
```

Leave `version` blank to exercise the installer’s normal latest-release lookup.
The release event and a versioned manual run fetch `cas-install.sh` from the
same tag being tested, then pass `CAS_VERSION` so the archive cannot silently
come from a different release.

## What the receipt proves

Both jobs upload a transcript artifact, even when their assertions fail.

- **macOS:** `macos-latest` must report `Darwin` and `arm64`. The real
  published installer runs with a clean HOME, wires `.zshenv` without a tty,
  verifies the archive against its published GitHub Release SHA-256 before
  extraction,
  proves `cas --version` through a fresh zsh login shell, checks that the
  installed binary has no `com.apple.quarantine` attribute, and runs bare
  `cas` plus `cas doctor` from another fresh login shell.
- **Linux:** `ubuntu-latest` runs an `ubuntu:24.04` container with only bash,
  curl, certificates, coreutils, grep, and tar installed. The job asserts that
  `rustc` is absent, then performs the same receipt-verified clean-HOME install
  and fresh bash login-shell checks.

The transcript greps the plain-language contracts as well as checking exit
codes: `Verified SHA-256 against the published GitHub release receipt.`,
`Cassy installed successfully!`, `A new terminal can run \`cas\`.`, the
fresh-machine `Welcome to Cassy. Run \`cas init\`...` hint, and the doctor
message that says to run `cas init`.

The receipt establishes corruption detection only. GitHub serves both the
archive and its per-asset digest under the repository's release-publishing
authority, so this check does not resist an actor able to substitute both.
There is currently no independent Cassy signing key or attestation identity;
claiming substitution resistance would first require naming and operating that
separate trust root.

## Release checklist and honest limits

Before announcing that a release is installable, retain the workflow URL and
both uploaded transcripts in the release record. The run must be green on both
jobs. Unit fixtures (`scripts/test-cas-install.sh`) and a successful archive
build are useful supporting checks, but are not substitutes for this receipt.

The hosted macOS job proves the scriptable quarantine surface: the binary that
the installer leaves behind executes without a manual `xattr -d` step. Hosted
runners do not reproduce every consumer Mac condition, including Finder's GUI
Gatekeeper prompt, the exact SIP configuration, or a user's local security
policy. A real-Mac manual checklist item remains: on a consumer Apple Silicon
Mac, run the published one-liner, record whether the first launch presents a
Gatekeeper dialog, and confirm that `cas --version`, bare `cas`, and `cas doctor`
work in a new terminal. That manual observation supplements rather than
replaces the two hosted transcripts.

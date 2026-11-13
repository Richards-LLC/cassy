### Changed — Release

- **macOS prebuild runs on the operator's Mac** — Was: every macOS release
  prebuild compiled all dependencies cold on hosted macOS (Build step 30.7
  min). Now: trusted merge-queue and `main` builds run on a self-hosted Apple
  Silicon runner with a persistent target directory and fall back to hosted
  macOS when that runner does not start a probe within 90 s.
  `scripts/install-mac-release-runner.sh` installs the runner as a launchd
  user agent.

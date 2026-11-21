### Fixed — Release

- **Release gate runs from macOS system bash** — `scripts/release-gate.sh` started
  under macOS `/bin/bash` 3.2 now re-runs itself once under the first bash 4 or
  later on PATH, in `/opt/homebrew/bin` or in `/usr/local/bin`. With none
  installed, it exits 2 with a one-line instruction to install bash instead of
  failing on `declare -A`.

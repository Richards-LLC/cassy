### Changed — macOS CI

- **Prefer an idle isolated Mac** — Trusted queue and release jobs can use prowl when its dedicated runner is online and idle, with hosted macOS fallback for busy, offline or unavailable inventory. Pull requests stay off prowl. A provisioning wizard installs and removes a non-admin launchd runner isolated from operator homes; existing checks and release stages remain unchanged.

### Fixed — Release validation

- **Source-only Commander delivery** — The queue accepts source lanes without generated bundle commits. Release-version and bundle changes still require committed assets to match a fresh build, including version-only changes that previously skipped web validation.

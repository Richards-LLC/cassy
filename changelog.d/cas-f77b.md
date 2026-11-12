### Fixed — Update

- **macOS keeps privacy grants across Cassy updates** — Was: every release was
  a new ad-hoc identity, so macOS asked again for permissions such as Desktop
  folder access after each update. Now: when the operator has created the
  local `Cassy Local Code Signing` identity, `cas update` re-signs the new
  binary with it, and later versions keep the grants.

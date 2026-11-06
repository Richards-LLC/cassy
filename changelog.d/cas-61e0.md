### Fixed — Release train handoff

- **Release follow-through** — Release preparation reuses the open train PR and
  returns after one admission check, with durable landing and publication wakes
  armed automatically. Published installs wake the owning supervisor once for
  post-release work, and failed notices retry without reinstalling. Receipts
  report first-child-to-main and main-to-publication times plus queue attempts.

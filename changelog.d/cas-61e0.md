### Fixed — Release train handoff

- **Release follow-through** — Release preparation reuses the open train PR and
  returns after one admission check, with durable landing and publication wakes
  armed automatically. Published installs wake the owning supervisor once for
  post-release work, and failed notices retry without reinstalling. Receipts
  report first-child-to-main and main-to-publication times plus queue attempts.
- **Hotfix exception** — A missing train, different train lane or unavailable
  tracking cannot block a hotfix cut through its original base branch. Pending
  auto-merge with failed required checks is reported as not admitted, and a
  version re-bump retires both old release watches.

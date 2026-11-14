### Fixed — Release

- **Release closeout no longer calls folded members reverted** — Was: the
  v4.28 closeout held 12 shipped members as `RELEASE_MEMBER_REVERTED`. A
  temporary commit the task reverted itself, a changelog or overview fragment
  draft rewritten before release prep folded it, or a Commander bundle release
  prep rebuilt each counted as a revert. Now: a member is reverted only when its
  net delivered change is missing from the release tree; rebuilt bundles are
  accepted once the member's source is proven present. A genuinely reverted
  member stays open.

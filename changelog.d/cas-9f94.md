### Fixed — Releases

- **Evolved members ship** — Was: a release cut refused with "zero would-close"
  when train siblings had later rewritten a member's lines, because preview held
  that member as reverted. Now: a member whose work is on the release commit and
  was only changed by later work closes; an explicit revert, a deleted file or
  restored pre-delivery content still holds it.

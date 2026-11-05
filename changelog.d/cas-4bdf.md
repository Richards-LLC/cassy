### Fixed — Releases

- **Require the fixes claimed by a release** — One Git-only check during preparation verifies task fragments and trailing task IDs on non-merge commits. It accepts equivalent cherry-picks despite later edits, ignores epic merge subjects and passing mentions, and reports every missing task together.

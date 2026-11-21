### Fixed — Release

- **Reuse proof when pushing an earlier commit** — Pre-push accepts a non-HEAD commit with matching fast-row and required web receipts for the push range. Missing proof still requires a clean checkout of that commit; refusal advice now points to the hook’s complete validation path.

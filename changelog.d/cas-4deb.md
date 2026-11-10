### Fixed — Slack files

- **Attach verified files to managed threads** — Artifact sharing resolves the caller's live reply intent, and the Violet CLI supports managed files above the inline limit. Both upload steps keep the same claim and request identity; missing or expired authority for a locally known managed thread is refused before upload.

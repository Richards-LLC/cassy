### Fixed — Pre-merge review

- **One verifier per dispatch** — the merge refusal no longer tells the supervisor to spawn a verifier while the automatic verifier holds the dispatch, and a verifier that finds its dispatch settled or its task closed raises no notice.

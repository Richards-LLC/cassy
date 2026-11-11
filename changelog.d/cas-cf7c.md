### Fixed — Release closeout

- **Replay stale closeout operations once** — Release receipts record the reconciling binary version. Older binaries refuse close effects, and an installed binary at or above the release tag replays unstamped or older-binary outcomes once while preserving delivery identity.

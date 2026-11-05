### Fixed — CI

- **Keep macOS runner waits out of queue admission** — Release queue entries no longer request the advisory macOS Check runner. GitHub-hosted macOS capacity gaps previously cancelled an unstarted required check and ejected green releases. Fast Validation is the sole queue gate; Release Prebuild still builds macOS and publication still audits its packaged artifact. Queue receipts no longer claim standalone macOS gate proof, and docs-only timing requires only Fast Validation.

### Fixed — Factory

- **Slack binding repair works on locally built binaries** — A factory daemon built from a tree with uncommitted edits reports its build as `<commit>-dirty`. Binding repair rejected that as "unknown or stale runtime identity", so reconciliation was refused on such builds. That build id is now accepted; a missing build id is still refused. The binding-repair tests now use a fixed runtime build instead of the test binary's own, and fail within 20 seconds instead of hanging for 10 minutes when repair never reaches the runtime.

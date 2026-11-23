### Fixed — Factory park

- **A lane that changes hub-web source parks with its source proof alone** — Was: the pre-push drift gate made the lane commit the rebuilt `hub-web/dist` bundles, and park then demanded a widened related-tests receipt for each changed bundle, which run no tests and could never earn one, so the lane could neither push without them nor park with them (GH #2329). Now: generated `hub-web/dist` files owe no proof and never widen; the `hub-web/src` files they were built from keep their normal proof.

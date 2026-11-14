### Fixed — Factory

- **Source-only Commander pushes no longer demand generated dist** — The pre-push hook requires committed Commander output to match a fresh build only where the merge queue does: a version change, a dist change or an unreadable version. An unchanged-version source lane pushes without committing `hub-web/dist` and leaves the checkout clean; release preparation still rebuilds and commits the bundle.

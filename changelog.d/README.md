# Changelog fragments

Each delivery adds its release entry here as its own file,
`changelog.d/<task-id>.md`, instead of editing `CHANGELOG.md`. Separate files
never share a hunk, so concurrent pull requests do not conflict in GitHub's
merge queue, which ignores the local `cas-changelog` merge driver.

A fragment is the body of an `## [Unreleased]` section: a category and area
heading, then one titled line per change.

```markdown
### Fixed — Commander

- **Status stays current** — Was: the status line lagged a reply. Now: it
  follows each live event.
```

The close gate accepts the fragment as the delivery's changelog entry, and the
`changelog-and-versions` release-gate row checks its format. `scripts/release.sh`
folds every fragment except this README into the dated version section and
deletes the files in the release commit.

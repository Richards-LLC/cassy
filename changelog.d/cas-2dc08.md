### Fixed — Release trains

- **Version check ignores longer numbers** — Was: the release gate's version-literals row matched the release version as plain text, so a Node `v24.x` path that happened to end in the release number failed every push on the train (GH #1865). Now: it matches the version only as a whole number; a version followed by a suffix such as `-rc.1` still fails.

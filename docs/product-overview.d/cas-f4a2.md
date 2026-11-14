## Core Concepts

- **Release members are judged by their net change** - the release closeout counts a member as reverted only when the change from its start base to its shipped commit is missing from the release tree. Commits the task undid itself, fragment drafts release prep folded, and rebuilt `hub-web/dist` bundles (once the member's source is present) do not count as reverts.

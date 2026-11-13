## Core Concepts

### Approval across train movement

When the train advances without changing a delivery's reviewed paths, the merge
retains its approved dispatch and records the target movement on the task. Clean
same-file merges retain the existing unchanged-patch approval path. If preparing
that candidate exceeds the response deadline, the merge stays held with its
approval intact until preparation finishes. A retry reuses that preparation;
changed delivery content and merge conflicts keep their existing review checks.

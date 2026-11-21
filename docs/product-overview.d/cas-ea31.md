## Core Concepts

Factory branch ownership follows registered worker identities. Temporary and task branches inherit their registered worker's namespace; a longer registered worker name takes precedence when names overlap. The worker keeps access to its own checkout and can switch back to its canonical branch while sibling checkout and branch protections remain enforced.

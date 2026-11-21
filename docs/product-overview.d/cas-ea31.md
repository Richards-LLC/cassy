## Core Concepts

Factory branch ownership follows registered worker identities. Temporary and task branches inherit their registered worker's namespace; a longer registered worker name takes precedence when names overlap. The worker keeps access to its own checkout and can switch back to its canonical branch while sibling checkout and branch protections remain enforced.

Installed worker pre-commit and pre-push hooks use the same registered ownership decision as the tool guard. Own temporary suffix branches work through both Git boundaries; overlapping sibling namespaces and cross-task destinations remain protected.

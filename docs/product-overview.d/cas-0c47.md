## Core Concepts

The director's 10-minute "CI unknown" merge decision applies only when CI could exist for the delivery. When every GitHub Actions workflow at the delivery tip provably skips a push to the factory branch (for example, pull-request-only workflows), the director raises no CI-unknown decision for that delivery. Unreadable workflows, an empty workflow set and filters it cannot evaluate still count as covered, so genuinely missing CI keeps its single bounded alert and clears once CI posts.

## Core Concepts

- **Release red from a refused cut** - when `scripts/release.sh` refuses to cut a train for REVIEW_MISSING, LANE_CI_MISSING or zero would-close, the cut's candidate binary runs `cas release refused` and records a release red for the green tip it started from. The release freeze then admits a `release-repair` task against that receipt, as it does for a train-suite red, without an operator lift; other merges into the frozen train stay refused and the train merge queue is not held. Recording is best effort and never changes the refusal.

### Fixed — Tasks

- **Derived priority refusals** — Task updates that request a priority conflicting with finding labels now return a typed error explaining the derivation and how to change it. The entire request is refused, so accompanying fields cannot save behind a misleading success response. Fixes #1781.

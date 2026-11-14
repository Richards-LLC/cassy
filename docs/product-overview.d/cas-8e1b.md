## Core Concepts

- **Candidate schema at the cut** — A release candidate can be newer than the store's schema. `cas release preview` then reads a private, migrated backup and leaves the live store unchanged. `cas release track` is the cut's first write, so it applies the candidate's migrations first and stops the cut if one fails. `scripts/release.sh` passes `CAS_ROOT` to both candidate calls.

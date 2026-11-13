## Core Concepts

- **Worker checkpoint pushes** — A pushed checkpoint on an owned InProgress task is not a parked delivery, including when commit-time branch and anchor capture exists. After a supervisor-directed target sync, the worker may rebase and publish using an explicit force-with-lease for the observed remote tip. H-40 verifies ownership, recorded start ancestry and authoritative delivery/review state; unknown state, deletion, parked work and rewritten reviewed anchors refuse. Parked or reviewed branches merge their target instead.

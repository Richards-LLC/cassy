## Core Concepts

Prepared release pushes and merge-queue entries reuse an ancestor's successful full-set proof when only release plumbing changed and main is unchanged. A durable adoption artifact records the release head/tree and original native producer head/run/attempt. Consumers verify both completed workflows, complete logs, source proof and the actual queue delta before suppressing suite jobs. Compile, launch, generated assets and Release Prebuild remain checked against the release tree. Any source edit or invalid proof keeps the full suite selected.

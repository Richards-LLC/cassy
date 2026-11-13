## Core Concepts

**Reworked peer deliveries.** When a peer re-parks a delivery, merge preparation follows its new recorded anchor if the local factory ref is an ancestor of the matching origin tip. Local divergence still requires explicit reconciliation. The update uses the existing checked-out-worktree safeguards and expected-old-SHA comparison.

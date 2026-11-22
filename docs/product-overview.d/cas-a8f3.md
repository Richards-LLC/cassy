## Core Concepts

Release membership reconciles lane CI that completed after an Advisory merge.
The release preview checks the immutable delivery anchor using the normal lane
CI classifier, accepting successful documentation checks with classified code
skips. It retains merge containment checks and leaves the historical admission
unchanged. Pending, failed, missing, and different-anchor CI cannot authorize
release membership.

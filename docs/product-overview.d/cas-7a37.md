## Core Concepts

- **Disk-pressure notice counts review scratch** - When builds wait on disk pressure, the notice to the supervisor now includes verifier review-scratch output queued for deletion in its reclaimable total, so it no longer says nothing can be freed while that output still fills the disk.

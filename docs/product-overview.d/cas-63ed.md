## Core Concepts

Worker recycling waits while a live detached build from the same worker registration and factory session holds its private target lease. The existing request resumes automatically after the lease releases, without a failure notice or manual retry. Ownership requires matching target provenance and producer process identity; unknown holders, completed producers and foreign-session runs retain visible lease refusals.

## Core Concepts

A worker recycle (`clear_context`) that meets brief store contention stays queued. The worker keeps running, and the next factory pass completes the recycle, so the supervisor does not have to request it again.

## Core Concepts

A worker with a spawn or recycle still in flight in the durable spawn queue is not idle. The director withholds its idle and pending-delivery notices until the replacement registers, then resumes the normal one-notice-per-episode behaviour.

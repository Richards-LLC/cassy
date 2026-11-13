## Core Concepts

**Cross-machine task ownership at session startup.**

A peer task can be InProgress without a lease on the observing machine. Session startup preserves that status and assignment instead of publishing an Open reset. Recovery of abandoned local work requires matching local lease history and applies only if the task snapshot remains current. Unknown ownership is preserved for supervisor review.

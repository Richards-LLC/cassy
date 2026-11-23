## Core Concepts

A worker's own pending reminder for its in-progress task is a declared wait. Stall detection skips the worker while that reminder is pending, the same way idle detection already did, and restarts its nudge-then-escalate ladder once the reminder has fired. A parked delivery (awaiting merge) or a blocked task is never treated as stalled.

## Core Concepts

A recorded capacity wait (an admission refusal with its scheduled retry, fenced to the worker's current task lease) counts as a worker wait, like a running build. The director withholds idle and detached-exit notices for it; the capacity-wait escalation after a long wait and notices for real check failures are unchanged.

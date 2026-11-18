## Core Concepts

An owned release train with merged work and no children in flight wakes its supervisor to run tip CI and freeze. Until recorded work changes, the watchdog repeats on its existing forty-five-minute cadence and pages the operator after ninety minutes. Frozen trains and trains owned by another supervisor remain excluded. Older trains with an explicit owner and release-train branch receive the same recovery.

Candidate preparation completion and failure notices use the existing authenticated supervisor wake path, so an idle supervisor receives the next action without waiting for an unrelated event.

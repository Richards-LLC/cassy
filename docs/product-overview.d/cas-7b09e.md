## Core Concepts

Worker task admission reconciles stale leases against terminal task state. A cancelled or closed predecessor cannot prevent the next assigned task from starting, including during queue repair. Unfinished assignments and claims still enforce one active task per worker.

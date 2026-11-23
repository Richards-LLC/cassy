## Core Concepts

- **Worker OS sandbox, per task** - a sandboxed worker's Bash writes reach only its current tasks' artifact directories and its own checkout's scratch, never a sibling task's evidence. The profile is fixed at launch, so starting a task it does not cover relaunches the worker with a new profile; the task stays started and the relaunched worker resumes it.

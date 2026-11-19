## Core Concepts

- `/standby` parks a factory supervisor session: it runs the finish flow, stops every worker through the normal shutdown path (never forced), records a standby marker (workers, held tasks, current handoff) and goes quiet. While parked, the daemon wakes the supervisor pane only for the operator and CAS lifecycle events; other messages wait in its inbox. `factory action=standby status=resume` respawns a replacement for each worker that held tasks, from its task queue, and clears the marker; the supervisor's first operator message does the same through its prompt hook.

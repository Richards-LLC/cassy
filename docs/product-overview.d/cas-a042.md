## Core Concepts

Worker SessionStart omits the role-wide current worker handoff: a worker's assigned task brief and its task-artifact `HANDOFF.md` carry its work, so a handoff saved by another worker for a different task never becomes startup instructions. Supervisor handoffs keep their machine-scoped selection.

### Fixed — Session context

- **Workers no longer start with another worker's handoff** — Was: a worker's SessionStart injected the newest role-wide worker handoff, which could describe another worker's unrelated task. Now: worker startup omits it; the assigned task brief and the task's `HANDOFF.md` carry the work. Supervisor handoffs keep their machine-scoped selection.

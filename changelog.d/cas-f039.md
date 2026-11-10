### Fixed — Coordination

- **Recover expired Slack claims after an uncertain reply** — When Cloud identifies an expired claim held by a departed supervisor, Cassy asks Violet to read back and settle its unresolved reply before reclaiming the thread. Recovery preserves request identity across retries and leaves live owners and unproven reply outcomes held.

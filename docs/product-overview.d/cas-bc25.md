## Core Concepts

- **Repair class audit** - When a merge-queue ejection names failing tests, Cassy records them on the task. Sending that delivery back (`request_changes`) or spawning a repair worker for it then needs `class_audit`, a nextest filter for the whole test class that selects those tests. The class is stored on the task, and the worker's next park needs passing capped receipts at the tip that run every part of it. A task with no recorded failing tests is unaffected,.

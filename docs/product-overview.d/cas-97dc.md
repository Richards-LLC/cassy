## Core Concepts

- **Idle relays only report real deliveries** - The director no longer tells a supervisor that a worker has a pending delivery for a task that live state shows in progress, such as one just assigned, so a worker that has just started is not flagged for a message or reassignment.

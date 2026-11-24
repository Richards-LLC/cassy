## Core Concepts

- **Routed builds keep the worker recipe** - A build task routed to a peer machine carries the requester's `cli`, `model`, `effort` or `lane` in its route brief and `BUILD_ROUTE` note. The receiving `spawn_workers` for that task uses exactly that recipe, filling omitted fields and refusing a conflicting one; a route without a recipe is unchanged.

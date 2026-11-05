### Added — Factory

- **Place build overflow on a peer machine** — Supervisors publish builder slots, load and pressure in peer presence. When local admission is saturated, a new task can go to a fresh same-repository supervisor with capacity, with its full brief and a receipt naming free slots and load per CPU. Routes survive retries without duplicate asks or bouncing between machines. Set `factory.auto_route_builds=false` to disable automatic placement.

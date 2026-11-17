### Fixed — Factory

- **Node test files can supply stability proof** — `cas factory worker-check -- node --test <file>` runs the changed `node:test` cases five times in a row and records the receipt that park requires. Previously park demanded that proof for hub-web's `node:test` files while worker-check refused to run them.

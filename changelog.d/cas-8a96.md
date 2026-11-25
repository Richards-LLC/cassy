### Fixed — Factory

- **Park checks test-first evidence** — A passing run alone no longer parks a test-first task. Cassy requires a capped failing test at the start base or a committed red checkpoint, or a capped `cargo check --tests` failure at the start base.

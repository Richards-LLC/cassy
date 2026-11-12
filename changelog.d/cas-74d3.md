### Fixed — Factory

- **Intentional waits stay quiet** — The progress watchdog excludes tasks blocked by open dependencies and tasks assigned to held workers, preventing stall warnings and operator pages until work becomes actionable again.

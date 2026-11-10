### Fixed — Hub

- **Keep the hub running after an invalid restart address** — `cas hub restart` validates `--bind` before stopping the existing hub, so a refused address leaves Commander available.

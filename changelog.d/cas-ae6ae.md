### Fixed — CI

- **Lane impact no longer blames a delivery for a one-off failure** — A test that failed once at the delivery and passed once at its base is now reported as unattributed, not as caused by the delivery. It still fails the lane.

### Fixed — CI

- **Lane impact no longer blames a delivery for a one-off failure** — A test that fails at the delivery but passes at its base is rerun once at the delivery: only a repeat failure is reported as caused by the delivery; otherwise it is unattributed. Both still fail the lane.

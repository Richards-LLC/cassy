### Fixed — Developer tooling

- **A capped named-test check on a busy host is refused in seconds** — Was: `worker-check` spent about two minutes classifying changed tests on every capacity-wait retry before it checked build admission, then refused the same retry on builder cap or memory pressure, adding load to the saturated host. Now: a saturated host (every builder slot held, or memory pressure over the guard limit) refuses the check before any classification runs; the authoritative admission check is unchanged.

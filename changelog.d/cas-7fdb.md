### Fixed — Factory

- **Attribute inherited web failures before park** — A widened Playwright proof now replays failing journeys at the merge base using lane CI's attribution engine. An identical failure records its evidence and a release repair instead of blocking the delivery. Failures that pass at base, differ there, or cannot be attributed still block under G-61.

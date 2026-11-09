### Fixed — Factory

- **Frozen-train deliveries are not reported as a stall** — Was: the progress watchdog listed parked deliveries for a train frozen for release as owed by the supervisor, repeated the stall notice and threatened to page the operator, although only the operator can lift the freeze. Now: those deliveries leave the stall set; the supervisor hears once per freeze which tasks it holds, since when, and that the operator lifts it with Lift freeze in Commander. Release repairs still count as local work, and a lift makes the held deliveries local work again.

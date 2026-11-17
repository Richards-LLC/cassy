### Fixed — Factory

- **Deliveries carry their own completion wakes** — Parking a delivery, sending its merge request or waiting for CI automatically records durable supervisor wakes. CI reruns on the same commit re-arm by run attempt. The watchdog immediately reports a missing wake as a Cassy defect. Corrected delivery tips replace superseded automatic watches, and inherited epic targets use close-time resolution.

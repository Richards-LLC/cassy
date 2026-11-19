### Fixed — Release trains

- **Start the train suite at freeze** — Freezing immediately dispatches or adopts `full-validation.yml` at the current train tip and returns its run URL or an explicit skip/failure reason. Repeated freezes share the daemon's dispatch history, and a missing run shows no ETA. Release guidance and missing-receipt errors name the same workflow and its dispatch command.

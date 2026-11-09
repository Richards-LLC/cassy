### Fixed — Factory

- **Undelivered blocked alerts** — A lifecycle relay whose task moves on before supervisor delivery now queues a supervisor action before retiring. Failed alert writes keep the relay pending, and retries reuse one alert.

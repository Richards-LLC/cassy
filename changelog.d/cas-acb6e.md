### Fixed — Honest delay labels on late lifecycle messages

- **A hold recorded late no longer explains the whole delay** — A delayed lifecycle message used to name its last recorded hold as the cause of the entire delay, even when the hold began seconds before delivery. One 90-minute store-writer stall was reported as an "active composer" hold this way. Queue rows now record when their first hold began. When it began well after the message was queued, the label says how long the earlier wait went unrecorded.

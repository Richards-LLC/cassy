### Fixed — Violet replies

- **A post Violet never received says to retry it** — When a reply went out on a closed Violet connection and the one automatic resend also failed, Cassy answered `not_posted` with `retryable: false`. Supervisors then registered a second reply intent and left the first one open. A `not_posted` with reason `request_not_found` is now retryable and says to resend the same `request_id` (GH #1968).

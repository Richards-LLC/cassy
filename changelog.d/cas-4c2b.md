### Fixed — Violet replies

- **A post Violet has no record of says to resend it** — When a reply's outcome was uncertain and Violet's lookup had no record of the request, Cassy answered `not_posted` with `retryable: false`, so supervisors registered a second reply intent. That `not_posted` (reason `request_not_found`) is now retryable: Violet has no record of this request for this caller, so resend with the same request ID and arguments (GH #1968).

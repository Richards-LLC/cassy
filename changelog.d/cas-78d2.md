### Fixed — Close timeouts

- **A timed-out close keeps its refusal detail** — when a close outlived the response budget and was then refused, the CLOSE_OUTCOME task note recorded only the refusal heading. It now carries the refusal body (offending claim and remedy), capped at 600 bytes.

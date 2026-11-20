### Fixed — Factory

- **A busy factory no longer reports a verifier as failed to start** — Was: on a loaded Mac, the request to start a task's verifier gave up after four tries with "Resource temporarily unavailable (os error 35)", and the supervisor had to retry the close by hand, even though the verifier usually started a moment later. Now: a factory that does not answer within 5 seconds counts as busy, the request waits with growing pauses for up to 15 minutes, and a failure names the 5-second limit.

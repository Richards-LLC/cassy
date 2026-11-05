### Fixed — Factory

- **"Still reviewing" no longer releases a merge hold** — A peer's progress reply (`kind=status`, such as "reviewing, ETA 1h") to a supervisor's open question no longer counts as an answer. The merge it is about stays held, the same as after a bare "ok". Only an answer, or a reply with no kind, releases it.

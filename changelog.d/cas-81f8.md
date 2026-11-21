### Fixed — Factory

- **Operational reports post during release verification** — Was: a focused
  release epic could block an unrelated support PDF, and one verified report
  could mask a later held report in a batch. Now: each post waits only for its
  attributed epic, resolved from its epic, task or artifact record. Release
  announcements retain their separate post-release checks.

### Docs — Release preparation

- **Communication release inventory** — Was: the communication changes and newer main fixes had no combined release inventory. Now: the release draft, changelog coverage and module map identify the assembled changes and the remaining publication prerequisites.

### Fixed — Release integration

- **Keep the existing message contract intact** — Was: combining the shorter instructions with main left the served coordination schema nine bytes over its budget and shortened the cross-harness wake-policy sentence. Now: the schema retains main's compact description and every supervisor contract carries the same existing wake policy. Both regression guards remain unchanged.

### Tests — Image credential validation

- **Capture credentials before curl exits** — Was: the image-command credential check could inspect the process after it exited and fail despite unchanged source. Now: its fixture captures the arguments and credential-file permissions while the process is running.

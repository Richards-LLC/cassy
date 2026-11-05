### Docs — Release preparation

- **Communication release inventory** — Was: the communication changes and newer main fixes had no combined release inventory. Now: the release draft, changelog coverage and module map identify the assembled changes and the remaining publication prerequisites.

### Tests — Image credential validation

- **Capture credentials before curl exits** — Was: the image-command credential check could inspect the process after it exited and fail despite unchanged source. Now: its fixture captures the arguments and credential-file permissions while the process is running.

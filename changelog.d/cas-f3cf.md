### Fixed — Factory

- **A worker script cannot hand a file-read provider token to a child process** — The worker credential guard refuses (H-09) a script that reads any file into an environment's provider credential variable, by subscript, `update`, a dictionary or `dict(...)` keyword, and starts a child process with that environment or with the modified process environment. The file's location does not matter.

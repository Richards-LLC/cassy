### Fixed — Factory

- **A Codex that cannot run shell commands is refused before a worker is assigned** — Was: when Codex's code-mode host never started, a worker passed every login check, took its task, and then every shell command failed with "timed out negotiating with the code-mode host"; recycling relaunched it the same way, and the error read like an account problem. Now: spawn and recycle start the host first and refuse with its path and a reinstall remedy, stated as separate from the account login; recycle also refuses an account its login check reports logged out (GH #1870).

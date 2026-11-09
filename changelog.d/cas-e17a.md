### Fixed — Automatic pre-merge verifier finds the task's evidence

- **The automatic verifier is told where the task's evidence lives** — Was: the daemon-run pre-merge verifier had to derive `~/.cas/artifacts/<project-key>/<task-id>/` itself, guessed the wrong project key, and reported the task's LEDGER.md and test logs as "could not be located" while they existed. Now: its prompt names the task's resolved evidence directory, the same one the close gate reads, and asks it to name that absolute path if evidence is absent there.

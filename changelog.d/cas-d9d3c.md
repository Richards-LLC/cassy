### Changed — Factory

- **Release trains work for every project** — The "train ready to cut" notice names the project's own release or promotion command from `factory.release_command` (for example a staging-to-main promotion), and names `scripts/release.sh` only when the repository has it. Without either, it asks the supervisor to set the command.
- **Drop several fixes in one repair cycle** — `cas factory train drop <task>... --reason "<failure>"` reverts every named child in one step and re-enqueues the train once, so a second bad fix from the same queue run no longer reads as a second ejection. If a revert conflicts, nothing on the train changes and the error names the conflicting files and what to do next.

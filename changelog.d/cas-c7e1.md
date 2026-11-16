### Fixed — Factory

- **Train merges start the pre-merge verifier automatically** — A train merge entry that needs a verdict now asks the factory daemon to start its verifier during the resume pass, as a single merge does, instead of leaving a pending dispatch and a manual handoff that the active train refused. Retrying the train merge restarts a stopped verifier on the same dispatch, and recovery notices name the train merge while a train owns the target.

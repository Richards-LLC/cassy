### Developer — QA

- **A supported scratch QA path for the write-roots setter** — The operator-only setter for `factory.write_roots` now has a capped fixture that workers can run. In an isolated HOME and scratch project, it parses the real command and writes the scratch project's `config.toml` as the operator's pane would. It also checks that an agent tool call with the same command is still refused, as is a run from a worker process group. Nothing switches either refusal off. A capture of the installed binary stays an operator step; CONTRIBUTING documents both.

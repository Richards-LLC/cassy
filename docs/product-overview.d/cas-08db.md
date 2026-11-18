## Core Concepts

`factory.max_workers` in a project's `.cas/config.toml` limits that project's live workers. The same key in `~/.cas/config.toml` independently limits all live workers on the machine, defaulting to eight when unset. Leaving the project key unset adds no separate project cap. Spawn requests, including batches, must fit both limits. Worker status and refusals show the machine census plus any project cap; advertised peer capacity uses the smaller remaining allowance and the existing builder guard.

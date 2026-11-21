## Core Concepts

The macOS operator launch smoke submits temporary launchd jobs with an explicit clean environment, captures their terminal output and command exit status, and removes the jobs afterward. This lets release validation exercise operator launches independently of the factory process running the checks while retaining the agent refusal controls.

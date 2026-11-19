## Core Concepts

- **Missing epic branch at task start** - Starting a task whose epic target branch was never pushed or was deleted now refuses with the branch name and asks the supervisor to create or push it, instead of failing with an empty git start point; no task branch is created.

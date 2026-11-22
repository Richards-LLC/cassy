## Core Concepts

A named Commander journey prepares its source-bound snapshot before starting Playwright's webServer clock. Memory admission can wait normally; refusal reports NOT_STARTED with reason memory_admission and exit code 75, without claiming a journey failure. Ready snapshots are verified before serving, while fixture-only and registered-hub configurations avoid local snapshot builds.

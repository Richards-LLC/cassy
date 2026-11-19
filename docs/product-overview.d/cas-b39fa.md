## Core Concepts

Worker recycling preserves assignments through the gap between releasing the old process's lease and starting its replacement. Startup orphan cleanup respects pending lifecycle requests and live replacements in the same factory session. Codex context accounting reports usage only from artifacts updated since the current registration; an older rollout cannot establish the replacement's occupancy.

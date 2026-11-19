## Core Concepts

Worker recycling preserves assignments through the gap between releasing the old process's lease and starting its replacement. Startup orphan cleanup respects pending lifecycle requests and live replacements in the same factory session. Codex context accounting excludes artifacts older than the actual recycle boundary, recorded after shutdown and before relaunch for that factory session and worker. A worker that has not recycled keeps its own usage, including records written before registration.

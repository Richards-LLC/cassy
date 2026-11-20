## Core Concepts

- **Bounded conflict journal** - The sync conflict journal keeps the ten newest discarded-row copies per entity, plus the newest copy of each strategy that lifecycle guards, `cas doctor` collision reports and memory folds read. Older copies are removed when a new conflict for that entity is recorded; `cas cloud conflicts --prune <days>` still removes copies by age.

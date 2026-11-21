## Core Concepts

Capped worker checks validate physical TMPDIR ancestry before creating temporary output or starting Cargo. A `.git` file, directory or symlink in that ancestry refuses admission with H-50, independently of Git discovery ceilings. Existing safe scratch selection and per-worktree ownership checks remain in force.

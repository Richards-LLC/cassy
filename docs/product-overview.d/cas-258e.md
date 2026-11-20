## Core Concepts

- **Team sync settles each queued row once** - A personal sync row in a team project is cleared by the first team write the cloud confirms, so it is not re-sent every cycle. The conflict journal keeps one row per discarded state; a repeat that differs only in its revision number is not recorded again.

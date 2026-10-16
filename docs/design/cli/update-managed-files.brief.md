# cas update managed-file repair

| Field | Contract |
| --- | --- |
| First two lines | The managed-files receipt says current, committed, staged or conflicts, with the pending action where needed. |
| Scannable | One compact managed-files line per project; doctor has one managed file drift finding. |
| Readable | Staged changes name the existing staged work as the reason; conflicts remain untouched and JSON names their paths. |
| Machine output | Update adds managed_files with outcome, untracked, staged, commit and conflicts to its existing JSON receipt; doctor returns its existing checks shape. |
| Omitted | Builtin per-path inventories remain in JSON and existing verbose sync details; no push or unrelated project changes enter the repair commit. |

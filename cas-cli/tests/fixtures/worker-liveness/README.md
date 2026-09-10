# Worker liveness fixtures

Codex completion records were extracted from a real 2026-09-10 factory rollout that was incorrectly reported as having no observed completion. Only timestamps, event types and a redacted turn ID remain.

Claude tool-use and terminal turn_duration records were extracted from a real transcript. Only timestamps, record type, role, stop reason and content block type remain. Tests replay those shapes with controlled time and process samples, including a terminal stop-reason variant. No prompt, command, path, credential or tool output is retained.

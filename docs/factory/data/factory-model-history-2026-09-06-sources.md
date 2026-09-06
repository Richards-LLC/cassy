# Factory model history sources — 2026-09-06

## Source coverage and join evidence

- Database → transcript key: `project + worker_name`; worker name comes from `spawn_queue.spawn_worker` / `agents.name` and transcript `cwd` segment `.cas/worktrees/<worker>`. This is a bounded name join, not an inferred task join.
- Transcript files parsed: 1705; joined to DB worker rows: 1339; transcript-only sessions: 366; transcript→DB miss rate: 21.47%.
- DB session rows without a transcript match: 7590 (81.97% of emitted rows).
- Prices JSON: `loaded`. Blank `cost_usd` means the model has no supplied price entry; no price was inferred.

- `pippenz` DB `/home/pippenz/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 0 tasks; task notes source `tasks.notes`; factory logs `9` files/0 JSON lines.
- `Apps` DB `/home/pippenz/Apps/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 2 lease events, 698 tasks; task notes source `tasks.notes`; factory logs `2` files/0 JSON lines.
- `canvas` DB `/home/pippenz/Apps/canvas/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 693 tasks; task notes source `tasks.notes`; factory logs `1` files/0 JSON lines.
- `Penguinz` DB `/home/pippenz/Penguinz/.cas/cas.db`: 152 spawn rows (47 without a resolvable worker name skipped), 507 lease events, 1126 tasks; task notes source `tasks.notes`; factory logs `38` files/3063 JSON lines.
- `Accounting` DB `/home/pippenz/Petra Stella/Accounting/.cas/cas.db`: 25 spawn rows (18 without a resolvable worker name skipped), 152 lease events, 350 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `2022` DB `/home/pippenz/Petra Stella/Accounting/Roark Realty/2022/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 0 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `Petrastella` DB `/home/pippenz/Petrastella/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 693 tasks; task notes source `tasks.notes`; factory logs `2` files/0 JSON lines.
- `abundant-mines` DB `/home/pippenz/Petrastella/abundant-mines/.cas/cas.db`: 232 spawn rows (58 without a resolvable worker name skipped), 739 lease events, 1335 tasks; task notes source `tasks.notes`; factory logs `17` files/4104 JSON lines.
- `cas-src` DB `/home/pippenz/Petrastella/cas-src/.cas/cas.db`: 1009 spawn rows (179 without a resolvable worker name skipped), 4448 lease events, 2716 tasks; task notes source `tasks.notes`; factory logs `62` files/22756 JSON lines.
- `closure-club` DB `/home/pippenz/Petrastella/closure-club/.cas/cas.db`: 10 spawn rows (10 without a resolvable worker name skipped), 18 lease events, 13 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `country-liberty` DB `/home/pippenz/Petrastella/country-liberty/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 0 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `domdms` DB `/home/pippenz/Petrastella/domdms/.cas/cas.db`: 37 spawn rows (27 without a resolvable worker name skipped), 108 lease events, 258 tasks; task notes source `tasks.notes`; factory logs `4` files/0 JSON lines.
- `edws` DB `/home/pippenz/Petrastella/edws/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 0 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `fixy-quasar` DB `/home/pippenz/Petrastella/fixy-quasar/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 0 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `fixyrs` DB `/home/pippenz/Petrastella/fixyrs/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 0 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `full-package-media` DB `/home/pippenz/Petrastella/full-package-media/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 0 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `gabber-studio` DB `/home/pippenz/Petrastella/gabber-studio/.cas/cas.db`: 590 spawn rows (110 without a resolvable worker name skipped), 3031 lease events, 2805 tasks; task notes source `tasks.notes`; factory logs `56` files/10481 JSON lines.
- `git-mcp-server` DB `/home/pippenz/Petrastella/git-mcp-server/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 0 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `homeschool-whisper` DB `/home/pippenz/Petrastella/homeschool-whisper/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 0 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `ozer` DB `/home/pippenz/Petrastella/ozer/.cas/cas.db`: 342 spawn rows (149 without a resolvable worker name skipped), 1832 lease events, 2697 tasks; task notes source `tasks.notes`; factory logs `46` files/3476 JSON lines.
- `pantheon` DB `/home/pippenz/Petrastella/pantheon/.cas/cas.db`: 33 spawn rows (14 without a resolvable worker name skipped), 240 lease events, 279 tasks; task notes source `tasks.notes`; factory logs `9` files/91 JSON lines.
- `petra_stella_tools` DB `/home/pippenz/Petrastella/petra_stella_tools/.cas/cas.db`: 1 spawn rows (1 without a resolvable worker name skipped), 0 lease events, 693 tasks; task notes source `tasks.notes`; factory logs `2` files/0 JSON lines.
- `aws` DB `/home/pippenz/Petrastella/petra_stella_tools/aws/.cas/cas.db`: 2 spawn rows (0 without a resolvable worker name skipped), 2 lease events, 696 tasks; task notes source `tasks.notes`; factory logs `5` files/51 JSON lines.
- `logging` DB `/home/pippenz/Petrastella/petra_stella_tools/logging/.cas/cas.db`: 2 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 6 tasks; task notes source `tasks.notes`; factory logs `4` files/0 JSON lines.
- `mecha_cassy` DB `/home/pippenz/Petrastella/petra_stella_tools/mecha_cassy/.cas/cas.db`: 20 spawn rows (1 without a resolvable worker name skipped), 80 lease events, 817 tasks; task notes source `tasks.notes`; factory logs `7` files/679 JSON lines.
- `petra-stella-cloud` DB `/home/pippenz/Petrastella/petra_stella_tools/petra-stella-cloud/.cas/cas.db`: 132 spawn rows (19 without a resolvable worker name skipped), 484 lease events, 382 tasks; task notes source `tasks.notes`; factory logs `26` files/2577 JSON lines.
- `project-management` DB `/home/pippenz/Petrastella/petra_stella_tools/project-management/.cas/cas.db`: 13 spawn rows (13 without a resolvable worker name skipped), 58 lease events, 38 tasks; task notes source `tasks.notes`; factory logs `4` files/314 JSON lines.
- `time-tracking` DB `/home/pippenz/Petrastella/petra_stella_tools/time-tracking/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 6 lease events, 227 tasks; task notes source `tasks.notes`; factory logs `2` files/0 JSON lines.
- `pixel-hive` DB `/home/pippenz/Petrastella/pixel-hive/.cas/cas.db`: 9 spawn rows (0 without a resolvable worker name skipped), 42 lease events, 29 tasks; task notes source `tasks.notes`; factory logs `4` files/93 JSON lines.
- `prospect_path` DB `/home/pippenz/Petrastella/prospect_path/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 0 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `pulse-card` DB `/home/pippenz/Petrastella/pulse-card/.cas/cas.db`: 39 spawn rows (0 without a resolvable worker name skipped), 178 lease events, 877 tasks; task notes source `tasks.notes`; factory logs `10` files/864 JSON lines.
- `rocketship-template` DB `/home/pippenz/Petrastella/rocketship-template/.cas/cas.db`: 155 spawn rows (20 without a resolvable worker name skipped), 1064 lease events, 4614 tasks; task notes source `tasks.notes`; factory logs `11` files/5192 JSON lines.
- `tracetix` DB `/home/pippenz/Petrastella/tracetix/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 0 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `verified-path` DB `/home/pippenz/Petrastella/verified-path/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 0 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `Accounting` DB `/home/pippenz/Richards LLC/Accounting/.cas/cas.db`: 30 spawn rows (22 without a resolvable worker name skipped), 248 lease events, 1170 tasks; task notes source `tasks.notes`; factory logs `6` files/114 JSON lines.
- `2022` DB `/home/pippenz/Richards LLC/Accounting/Roark Realty/2022/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 0 tasks; task notes source `tasks.notes`; factory logs `3` files/0 JSON lines.
- `Woodworking` DB `/home/pippenz/Woodworking/.cas/cas.db`: 63 spawn rows (42 without a resolvable worker name skipped), 240 lease events, 684 tasks; task notes source `tasks.notes`; factory logs `11` files/1286 JSON lines.
- `hermes` DB `/home/pippenz/ai/hermes/.cas/cas.db`: 2 spawn rows (0 without a resolvable worker name skipped), 16 lease events, 7 tasks; task notes source `tasks.notes`; factory logs `4` files/102 JSON lines.
- `ai-toolkit` DB `/home/pippenz/ai-toolkit/.cas/cas.db`: 0 spawn rows (0 without a resolvable worker name skipped), 0 lease events, 693 tasks; task notes source `tasks.notes`; factory logs `1` files/0 JSON lines.
- `soundwave-config` DB `/home/pippenz/soundwave-config/.cas/cas.db`: 18 spawn rows (1 without a resolvable worker name skipped), 98 lease events, 831 tasks; task notes source `tasks.notes`; factory logs `6` files/486 JSON lines.

### Field policy

- `task_notes` is not a table in the observed stores. Counts use the full `tasks.notes` field when present, falling back to `events.summary` only for `task_note_added` events; no note text is reconstructed beyond those stores.
- Exact first-push time is populated only when a JSON factory log record names the worker (and task when available) and contains a positive `pushed` marker. Missing markers remain blank; commit time is not substituted.
- Codex usage sums modern `token_usage_record.payload.usage` records or legacy `event_msg.payload.info.last_token_usage` records (modern records win if both exist); Claude usage sums assistant `message.usage`. Cache-read and cache-creation are retained separately, and reasoning comes only from an explicit usage field.
- Transcript task IDs are unavailable in the transcript sources, so task attribution comes only from DB spawn/lease rows. Transcript-only rows intentionally have a blank task ID.

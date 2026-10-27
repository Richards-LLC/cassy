# Subscription reset times

- Reader: the operator comparing Claude and Codex availability before launching or recovering a worker.
- Decision: see the exact local day/time for every known 5h and weekly reset beside its relative wait; missing or rolled-over usage stays explicit.
- Surface: one limits-layer formatter feeds account lists, picker details and reasons, Commander account rows, worker status and recovery notices. Inquire options use multiple physical lines and height-based paging. Existing Commander secondary text wraps in its account row.
- Machine output: stored and JSON reset instants remain UTC RFC3339. Launch profiles gain optional display-only usage text; no browser-side date formatter. Status uses already-known/local data and never fetches live usage.
- Constraints: 80 columns; reset values are not truncated. Per supervisor #4211474 the factory compact strip stays one line, shows only each account's most constraining window and absolute reset, orders by soonest reset and ends with +N more when full. Expired-window wording is unchanged.

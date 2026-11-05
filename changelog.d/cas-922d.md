### Fixed — Factory

- **Urgent broadcasts wait for each worker's in-flight tool call** — An urgent message to all workers no longer presses Escape into a running tool call. Each such worker gets the message when its call ends. After 60 s it is interrupted with "Cassy interrupted your in-flight tool call to deliver this; this was not the operator."
- **Worker asks and answers wake the supervisor at once** — A worker's `kind=ask` or `kind=answer` message now keeps its kind. The six-per-hour wake cap no longer holds it, so a seventh ask in an hour no longer waits up to an hour. Asks, answers and blockers also skip the 20-second burst debounce, so an idle supervisor wakes on the next queue pass (about 0.1 s) instead of after about 20 s.
- **Declined supervisor wakes recheck within 30 s** — The backoff after a declined wake is now 10, 20, then 30 s, capped at 30 s instead of 2 minutes.

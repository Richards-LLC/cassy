# CI red-run relays

While a factory daemon is running, it checks completed GitHub Actions runs once
per minute. The list request costs at most 60 GitHub API calls per hour; a
completed failure adds one jobs request and an optional failed-log lookup.

The watcher considers `main` and the `factory/<worker>` branches for workers
currently live in that factory. Successful runs stay silent. A failure creates
one lifecycle-wake relay per `(branch, head SHA)`, carrying the run URL, failing
job, and the first Rust test name when the failed log has the usual `test …
... FAILED` form. The prompt queue's durable idempotency key survives daemon
restarts, so repeated polls never alarm twice for the same red commit.

The watcher uses the existing `gh` CLI credentials. If the CLI, GitHub origin,
or authentication is unavailable, the daemon logs one warning and retries
silently at later cadences; factory operation continues normally.

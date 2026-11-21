## Core Concepts

### Parked close ownership during worker shutdown

A committed park can answer before evidence, owner publication and its outcome note finish. Worker shutdown, forced shutdown and recycle refuse while that close still owns its process. Queued shutdown rechecks at the actual teardown boundary and preserves task bindings and live mail on refusal.

If owner publication fails after the park, the outcome records that failure and alerts the supervisor. A durable close journal retries only owner publication during daemon recovery, using the existing outbox deduplication key. Recovery stops when that delivery has been superseded; it never repeats task close.

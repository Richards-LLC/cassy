## Factory Coordination

Reminder admission tolerates transient write contention from other factory processes. A `coordination action=remind` request retries a held write lock within the store's bounded write budget and records the reminder exactly once; persistent contention is refused with no reminder stored.

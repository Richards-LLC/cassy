### Changed — Factory

- **Ejected deliveries go straight back to their worker** — When a merge-queue run fails and ejects a delivery PR, Cassy now returns the task Open to the same worker, with the failing job, step and failing tests as the change request, and wakes that worker. The supervisor no longer has to run request changes by hand. A task gets one such repair cycle; a second failed ejection leaves it parked and pages the operator. Dequeues without a failed run change nothing.

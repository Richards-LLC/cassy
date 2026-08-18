# Slack draft — concurrent factory session isolation (2026-07-06)

Channel: #cas-internal (`C0B44GUKDK2`) — two top-level posts.

---

## Post 1 — User

Running two Cassy factories on the same project used to be chaos: one session's workers would appear inside the other's terminal, a supervisor could accidentally shut down the *other* session's workers, and even a plain Claude session in the same folder would get poked by a factory it had nothing to do with. Now every factory session is its own sealed room.

- Spin up as many sessions on one project as you like — each one only spawns, sees, messages, and shuts down its **own** workers.
- A plain (non-factory) Claude session in the same project is invisible to factories: no more surprise nudges or status pings meant for someone else's workers.
- Messages between a supervisor and its workers stay inside that session — even if two sessions happen to have workers with the same name.
- Nothing changes for the single-session case, and everything that existed before the upgrade keeps working as-is.

## Post 2 — Dev

The coordination layer (spawn queue, agent registry, message queue in the project DB) was project-keyed: any attached factory daemon polled and consumed every session's spawn/shutdown requests, directors enumerated all project agents, and message delivery matched on target name alone. Now every factory-scoped resource is keyed by the owning session.

- `spawn_queue` and `agents` gain a nullable `factory_session` column (idempotent migrations with detect guards + index); all enqueue/registration paths stamp it from the session env; polls filter to own-session plus legacy NULL rows.
- Directors and the factory MCP ops (`worker_status`, `worker_activity`, shutdown, sync) enumerate only their own session's agents via a single shared `Agent::visible_to_factory_session` predicate; plain non-factory agents (NULL tag) are never visible to a factory director.
- Message delivery: session-tagged rows deliver only to the matching session's daemon — target-name collisions and `all_workers` broadcasts no longer cross sessions; untagged legacy rows keep the historical matching path.
- `worker_activity` also fixes two older defects: the `target=` filter now actually filters, and Idle workers' activity is no longer hidden.
- Hardening from the epic review: a registration/update can never implicitly downgrade an existing session tag to NULL (`COALESCE` on the conflict path, regression-tested), and a two-session integration test asserts disjoint spawn execution, agent visibility, pending-message counts, and delivery under name collision.
- Upgrade note: agents registered by an older binary carry a NULL tag until they re-register — restart the factory after updating.

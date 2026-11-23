### Fixed — Violet replies

- **A mention wake names the thread it came from** — A Violet mention wake on a managed thread now carries the thread's `resource_id` and the claim ref to run first, and names `factory action=reply_intent` with those ids and the wake's `occurrence_id`. Before, it gave only `delivery_id` and `occurrence_id` and pointed to `coordination reply_intent`, which does not exist, so supervisors passed `delivery_id` as the resource and got `unknown_resource` (GH #2242).

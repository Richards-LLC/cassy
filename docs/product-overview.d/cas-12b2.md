## Primary Journeys

When a project's Codex workers route through the home hub gateway, `spawn_workers lane=…` verifies each Codex recipe's model against the hub's model listing instead of the local account's `auth.json` and `models_cache.json`; a worker that would start direct still uses its account's own catalogue.

## Core Concepts

- **Fresh hub quota without traffic** - Besides the rate-limit signals on proxied requests, Accounts and New session read the hub's latest completed codex-inspection run. Its timer reads each account through the provider's zero-cost usage endpoint. For each login the newer of the two reads wins, so a Codex login stays current while no Codex work routes through the hub, and every machine still shows one number. Cassy reads the runs list and one run by id with the dashboard's read-only key; starting or cancelling a run is refused.

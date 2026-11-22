## Core Concepts

Spawn keeps worker namespaces exclusive across machines too. A new worker name is refused (`WORKER_NAME_OVERLAPS`) when it would share a `factory/` namespace with a registered worker or with an existing local or origin `factory/*` branch. A name's own task branches never block it. Generated names skip such candidates, and overflow minion names read `kevin2`, not `kevin-2`. `cas doctor` reports any overlap that already exists in a `worker names` row, with the way back, and changes nothing.

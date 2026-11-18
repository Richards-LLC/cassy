## Primary Journeys

### Cloud authorization and isolated proof checks

Proof publication, peer messaging, message-status reads and agent discovery retry one 401 response that identifies an invalid API key with the current stored login or a verified environment credential. The environment credential is scoped to its configured endpoint and shares the direct-token login path. Other 401 responses retain the server cause without retrying. Failed credential recovery reports `Cloud session expired, run cas login`; transport failures and uncertain writes do not trigger this retry.

Capped tests use short scratch paths outside a checkout and protected `.cas` ancestry. The credential-fixture sentinel also covers arbitrary login tokens, so an intended no-project fixture cannot overwrite the operator's project or user login cache.

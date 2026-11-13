## Core Concepts

Peer replies with an exact Cloud `in_reply_to` resolve the sender's outgoing ask
before wake-kind filtering. This includes quiet answers to automatic build-route
asks. Messages without an explicit link keep the existing correlation rules;
quiet, status and receipt messages do not implicitly answer questions. Structured
`CAS_MERGE_HOLD` task and PR scopes continue to govern which asks hold merges.

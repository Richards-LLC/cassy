## Core Concepts

Team sync includes known server revisions on upserts and records accepted revisions. Rejected writes retain their known base and queued edit until reconciliation. Personal and team pulls refresh queued bodies together with the observed revision, including eligible local-wins task guards; task notes merge independently of lifecycle. A revision conflict waits visibly for reconciliation and is rearmed after that succeeds. Foreign, malformed or unresolved delivery conflicts cannot authorize a fresh base. Atomic merging of concurrent task-note appends remains owned by petra-stella-cloud#268; this client change does not claim to implement that server operation.

Artifact lookup distinguishes missing publication metadata from byte availability. If neither the local ledger nor synced task notes contain the publication reference, the diagnostic directs investigation to the publisher's Cloud reference instead of promising that another receiver sync will repair it.

## Core Concepts

Team sync includes known server revisions on upserts and records accepted revisions. Rejected writes retain their known base and queued edit until reconciliation. Atomic merging of concurrent task-note appends remains owned by petra-stella-cloud#268; this client change does not claim to implement that server operation.

Artifact lookup distinguishes missing publication metadata from byte availability. If neither the local ledger nor synced task notes contain the publication reference, the diagnostic directs investigation to the publisher's Cloud reference instead of promising that another receiver sync will repair it.

## Core Concepts

### Parallel full-set validation

The Rust archive feeds six disjoint suite shards, with complete shard evidence
required by the collector. Classification also handles runner routing and tree
reuse. The hosted full-set gate collects native reports, publishes repair issues
and rejects failed or cancelled producers, publication and queue checks. Its
receipt binds those outcomes to the evaluated tree. A separate Fast Validation
wrapper preserves the protected branch's required job check.

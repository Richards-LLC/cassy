### Added — Task close

- **Operator recovery decisions for contained historical deliveries** — a
  delivery already on its target with no reusable pre-merge verdict could not
  close, and when a later target commit had superseded one of its paths, the
  `reviewed-drop:` reason and the missing-approval check each refused the
  other (GH #2270). The operator now runs one command per epic,
  `cas task recovery-decision --scope <epic-id> --note "..."`. It prints and
  records the checks it waives: G-56 reviewed drop (the named commits must
  still cover every dropped path), G-64 independent QA and G-70 missing
  approval. A live supervisor then closes each contained delivery with
  `operator_decision=<id>` and `commit_receipt=<containment commit>`, without
  `supervisor_override`. G-164 refuses an unknown decision, a decision for
  another epic, a combined `supervisor_override` and a delivery the decision
  cannot close. Tier 2 deliveries, and deliveries whose tier cannot be
  measured, keep every check. Agents cannot run the command or write its
  record.

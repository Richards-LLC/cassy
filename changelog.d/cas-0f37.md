### Fixed — Task close

- **One supervisor close recovers an integrated historical delivery** — a
  delivery already on its target with no reusable pre-merge verdict, and with
  a path a later target commit superseded, could not close: the
  `reviewed-drop:` reason passed the content check and then failed the
  missing-approval check, and the reason that check suggested failed the
  content check. A live supervisor's `supervisor_override=true` close whose
  `commit_receipt` is on the target now records the recovery decision, so one
  close satisfies both. Unintegrated receipts and unreviewed content loss
  still refuse, and Tier 2 deliveries keep their adversarial review (GH #2270).

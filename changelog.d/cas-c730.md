### Fixed — Factory

- **A returned delivery's start names the tip it must keep** — Was: after a
  parked delivery came back on a merge conflict, its start listed only the
  pushed-tip requirement, and the push guard let a rebase force-push over the
  reviewed tip. Now: the start's close checks list the returned delivery tip
  and say to merge the target in, and the push guard refuses any rewrite that
  drops that tip. Ordinary unparked checkpoints can still be rebased.

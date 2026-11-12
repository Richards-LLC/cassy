### Changed — Factory

- **One delivery identity check per question** — Was: an approval could be
  rebound to a new candidate when a target move touched a delivered file but
  the delivery's own patch-id (minus five generated paths) stayed equal;
  rebased deliveries were compared commit by commit; release membership read
  three anchor orders; a member's generation was the task row's last edit.
  Now: the only pre-merge carry is the candidate tree still validating, so a
  target move on a delivered path gets a fresh review. A rebased delivery is
  the same delivery when its whole change has the same patch-id (merge
  commits and other tasks' commits still refuse). Membership uses the one
  delivery anchor, and a member's generation is its delivery cycle.
  Generated output is excluded through one list.

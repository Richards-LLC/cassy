### Frozen train repair admission

For an active release freeze, merge admission requires the repair's exact SHA
  to pass the lane's fast and factory/PR checks. Rust impact jobs defer to the
  full-set run at the new frozen train tip. Other job failures and pending work
  retain their normal verdicts. Missing freeze or check evidence retains normal
  lane admission. Existing freeze, repair authorization and main landing gates
  still apply.

After publishing the merge, Cassy makes a bounded attempt to cancel that SHA's
  factory lane when its only unfinished jobs are Rust impact jobs. Cancellation
  never targets the train's full-set workflow, and failure to cancel does not
  undo the merge.

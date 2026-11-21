### Fixed — Factory

- **Lane re-push after a train merge no longer demands a dist commit** — the
  pre-push fast rows compared an existing lane with the tip it replaced, so a
  train's own `hub-web/dist` rebuild merged into the lane counted as the lane's
  change and required a lane dist commit the merge policy forbids. A re-push
  that absorbed newer target commits now compares with the task target's
  merge-base, as a first push does.

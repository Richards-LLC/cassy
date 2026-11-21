### Fixed — Factory

- **Parked deliveries close once their work lands on any target** — Was: a
  delivery merged outside `worktree_merge` (a PR into develop, a manual merge
  into an epic branch, a develop→staging promotion) stayed parked, and with no
  pre-merge verdict it could close only through a per-task override. Now: the
  daemon sees the delivery on its recorded target and closes it on an existing
  approval. With no verdict, it starts the post-merge verifier (at most four at
  once) and closes the task when that verifier approves. The PR observer now
  watches every target branch, not only main. Deliveries not yet on their
  target still need a pre-merge verdict.

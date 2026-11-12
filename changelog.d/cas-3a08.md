### Fixed — Factory

- **Checkout refusals name the checkout fix** — Was: a worker `git checkout`
  outside its assigned worktree was refused with commit guidance ("switch to
  your lane, then retry your commit"). Now: the refusal
  (`WORKER_CHECKOUT_OUTSIDE_WORKTREE`) gives the checkout to run inside the
  assigned worktree, for a revision or a path restore (`git checkout -- <path>`).
  Commit refusals keep their commit guidance, and the ownership rule is
  unchanged.

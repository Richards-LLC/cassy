### Fixed — Release train

- **One train-suite run per frozen tip** — Was: when a run was already under
  way at the frozen tip, the daemon dispatched a second one, and a newer
  cancelled duplicate hid the green receipt from the cut. Now: the daemon
  dispatches only when no run is queued or running at that tip, and a
  cancelled run never outranks a finished one at the same tip.

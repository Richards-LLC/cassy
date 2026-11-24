### Changed — Factory

- **Capped runs queue for a builder slot** — Was: a backgrounded capped test
  or check that met a full builder cap, CPU/memory pressure or low disk exited
  at once, woke its worker, and told it to retry in 120 seconds, so waiting for
  a build cost two wakes per retry. Now: the run waits in its own session,
  keeps its place in the priority queue, starts when admitted and wakes its
  worker once with the result. A run killed while waiting is reported once. A
  foreground run is still refused, and the refusal names the backgrounded
  command to run instead.

### Fixed — Factory

- **An approved no-code review closes the task by itself** — Was: when a
  release-train worker's no-code or no-delivery close was approved by Cassy's
  automatic verifier, the close was refused because the verifier, not the
  train's owner, ran it. The task stayed Blocked until a supervisor repeated
  the same close by hand, and the five-minute alert only said the close was
  "still held". Now: Cassy runs the close for the review's owning supervisor,
  so the task closes itself. If a real gate still holds it, the gate's code and
  message are saved on the task, and the alert names that gate.

### Fixed — Cloud sync

- **The conflict log stays small** — Was: every sync conflict saved another
  full copy of the local row, forever, and one project database grew to
  22 GB. Now: each task or memory keeps its ten most recent conflict copies,
  plus the latest copy of each conflict kind that Cassy still reads.

### Fixed — Factory

- **Old session-end records and delivered operator messages are cleaned up** —
  Was: every ended session kept its full hand-off record (a supervisor's can
  be several megabytes), and every message to the operator kept a delivery
  copy that also stopped the message itself from ever being cleaned up. Now:
  the regular retention pass removes records older than the existing retention
  windows once nothing still needs them; pending hand-offs, live successors
  and undelivered messages are always kept.

### Fixed — Factory capacity

- **Apply worker caps to the correct scope** — A project's worker cap now counts only workers in that project; the user-level cap still counts every live worker on the machine. A project at three of eight workers can staff another worker when the machine is at ten of twenty. Refusals and status show each applicable limit, and peer capacity respects both.

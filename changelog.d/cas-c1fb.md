### Fixed — Factory

- **Local stall pages respect peer release waits** — The progress watchdog excludes parked deliveries already contained in their target or assigned to a peer through a binding epic split. Genuine local stalls still wake the supervisor and page the operator.

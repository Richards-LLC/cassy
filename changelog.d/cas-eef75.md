### Fixed — Pre-merge verification

- **A held merge that lands on approval now wakes its supervisor** — Was: when the automatic verifier approved a held delivery, Cassy continued the merge and closed the task but sent the supervisor nothing, so the supervisor sat idle until asked and a production release waited 30 minutes. Now: the supervisor gets one wake per approval saying the merge landed and what state the task is in, so promotion, notes and dependents can start at once (GH #2365).

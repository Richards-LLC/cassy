### Fixed — Cloud task sync

- **Peer closes unblock dependencies** — A receipted close for the same approved and merged delivery now reaches a peer's dependency checks even after that peer appended a newer note. Newer local body and notes survive. Open work, changed delivery scope, explicit rework, foreign task IDs and a LocalWins policy continue to hold dependencies.

### Fixed — Factory

- **A delivery proven on one machine closes on the other** — Was: capped test receipts lived only in the worker machine's `.cas/worker-checks`, so a close on the supervising machine refused the blast-radius scope as untested and needed a supervisor override. Now: parking copies the worker's passing receipts at the parked tip onto the task row, which syncs with the task, and the close counts them when they match the delivered head, use a named filter and carry a log hash; a local receipt for the same test wins.

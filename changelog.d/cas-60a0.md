### Fixed — Factory

- **A successful `cas factory reexec` reports its receipt** — Was: on macOS, after the daemon swapped in place, the command could print `Invalid argument (os error 22)` with no receipt, because the new image closes the reply socket right after writing it and setting a read timeout on a closed socket fails. Now: the client reads the buffered receipt and reports the version, PID and adopted panes; a reexec that did not swap still returns its error.

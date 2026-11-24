### Fixed — Commander

- **Reload Cassy keeps a truthful status through a reconnect** — Was: while a
  confirmed Codex reload restarted the supervisor, a status check that met the
  reconnecting session showed "message was not sent. Retry…" as the reload's
  status, though the reload went on to finish. Now: a command the session
  already holds keeps its own waiting or restarting status while Commander
  reconnects, then shows the real outcome.

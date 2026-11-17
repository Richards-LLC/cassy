### Added — Releases

- **Supervisors can lift a release freeze** — Was: only the operator's Lift
  freeze in Commander reopened a frozen train, so a train frozen by auto-cut
  held its deliveries until the operator was at the keyboard, and auto-cut froze
  the lifted train again a minute later. Now: the train's supervisor lifts it
  with `train_freeze decision=lift` and a required reason, recorded on the train
  and sent to the operator; a freeze the operator applied stays theirs unless
  they set `release.supervisor_lifts_operator_freeze`, and auto-cut waits for a
  new tip after any lift.

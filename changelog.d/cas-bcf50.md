### Fixed — Commander

- **Conversations stay Live while they answer** — Was: when a machine's event
  stream reconnected, every conversation row turned gray "Last known" beside a
  footer saying Connected and a header saying Live, and sends waited up to two
  minutes before failing. Now: rows, machine chips and sends use the footer's
  rule, so a conversation whose session still answers reads Live and a send
  goes out at once.
- **Busy machines no longer refuse sends** — Was: when another task on the
  supervisor's machine held its database past two seconds, the send failed
  with "The supervisor's machine couldn't take the message". Now: the send
  waits and is queued, in order, as soon as the database frees up.

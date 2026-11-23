### Fixed — Factory

- **A fresh park is no longer reported as missing its completion wake** — Was: a delivery parked while the watchdog sampled was reported as `DELIVERY_WAKE_MISSING` about half a second before its own park finished arming its CI and merge wakes (the wakes existed by the time the notice was queued), as happened to a delivery moved with `transfer adopt_branch`. Now: the check waits 60 seconds after a delivery last changed, enough for its park to arm; a delivery that still has no wake after that is reported as before (GH #2332).

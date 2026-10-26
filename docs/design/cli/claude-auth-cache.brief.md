# Claude auth cache recovery advice

| Field | Contract |
| --- | --- |
| First two lines | The existing doctor/preflight verdict identifies a logged-out Claude account; its remedy must disclose that cached status can take one minute to refresh after login. |
| Scannable | Keep the existing capability row and remedy order; this change adds the wait to the existing login instruction. |
| Readable | Say “Run `claude login`, then retry doctor after one minute.” so an immediate cached result does not contradict the remedy. |
| Machine output | Capability evidence keeps `availability`, `observed_at_ms`, `ttl_ms`, `reason`, and `remediation`; the remedy text states the same delay in JSON. |
| Omitted | No login interception, new command, credential content, or cache internals appear in the remedy. |

The logged-out evidence TTL and minimum Claude probe interval are both 60 seconds.
A cached Unknown result keeps its shorter freshness TTL while the existing probe
throttle still waits one minute. A user-written TTL cannot extend either state's
normal freshness or the probe's bounded interval.

Terminal captures and critique for this narrow wording change belong to the
cas-b15e task artifact directory and must name the supervisor-built candidate.

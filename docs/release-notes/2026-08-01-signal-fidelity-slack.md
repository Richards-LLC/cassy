# 2026-08-01 — signal fidelity: delivery, spawn, dependencies, scoping — #cas-internal posts

Covers the late 2026-07-31 delivery/identity fixes (`fd328c68`) and the
2026-08-01 lifecycle-integrity work (`f4abfcdb`). One story, not two.

## Post 1 — User

**Live on production — User**

Was: you could start something, be told it worked, and only discover twenty minutes later that nothing ever happened — because every status display kept insisting things were fine. Now: when something fails to start it says so at the time, and when it reports work in progress that claim is backed by evidence instead of optimism.

- **Was:** a message sent to a session that was sitting idle could vanish silently — the text arrived but was never actually submitted. **Now:** it lands and gets picked up.
- **Was:** starting several sessions at once could quietly cancel some of them, with no error recorded anywhere. **Now:** every start is tracked through to completion, and one that dies on the way is reported rather than forgotten.
- **Was:** you'd get "waiting for work" notifications about sessions that had already started working. **Now:** those stale alerts are dropped before they reach you.
- **Was:** work could begin on something that was still waiting on unfinished prerequisites. **Now:** it refuses to start and names exactly what it's waiting for.
- **Was:** a sync could quietly point at an unrelated project's branch. **Now:** it stays inside the project you asked about, or stops and explains why it can't.
- **Was:** "did it actually see my message?" had no answer — the status looked identical whether it responded instantly or never woke up at all. **Now:** you get a real answer, drawn from what actually happened.
- **Was:** running the test suite could scribble on your real configuration and leave stray processes tying up your database. **Now:** tests stay in their own sandbox.

## Post 2 — Dev

**Live on production — Dev**

Was: several control-plane operations returned success-shaped results over state that was either wrong or fundamentally unknowable. Now: they resolve before they mutate, fail closed with actionable detail, and report evidence rather than inference.

- **Was:** terminal injection into an idle session wrote the payload but never submitted it — the paste-burst detector was swallowing the trailing carriage return. **Now:** injection is framed as a bracketed paste, so the submit survives.
- **Was:** spawn cancellation was request-order blind; a shutdown-all could jump FIFO and cancel newer in-flight generations that happened to be polled in the same batch, which is why this presented as intermittent across unrelated projects. **Now:** requests carry monotonic ids and cancellation is bounded to generations predating the shutdown. Dequeue, provision, launch and register are each logged, and a child that exits before registration surfaces as a failure event instead of silence.
- **Was:** start and claim never evaluated live blocking-dependency rows, so work could begin on a blocked item. **Now:** both fail closed and name the specific blocking ids; adding a blocking edge re-arms an open item. Non-blocking relationship types explicitly cannot block or re-arm.
- **Was:** sync target resolution fell back to the first ready item it found across an unscoped listing, which could select another project's branch entirely. **Now:** an explicit id is authoritative, resolution is scoped to the current project, and the target resolves completely before any clone is touched. The unscoped fallback is removed rather than filtered.
- **Was:** wake and reaction were structurally unobservable — a single-variant status that read identically for an instant response and for one that never arrived. **Now:** observation is artifact-backed and read-only, driven by exact queued-prompt and turn correlation. Inbox persistence stays delivery evidence only, turn-end stays completion rather than wake, and elapsed time never promotes an unobserved record. No retry or escalation logic was added to the delivery path.
- **Was:** integration tests performed host-registry writes against the real configuration directory, causing lock contention and long-lived orphan processes. **Now:** that registration happens only on the real initialization paths.
- **Was:** replies addressed back to the originating session could fail to resolve, stranding finished work in a waiting state. **Now:** the originating identity is stamped at start-up from the trusted path, so the return route resolves.

Gate: 116 targets, 5837 tests, zero failures.

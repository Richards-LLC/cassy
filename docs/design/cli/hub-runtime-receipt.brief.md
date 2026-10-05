# Hub runtime receipt

## Reader

An operator or diagnostic consumer checking what recovered after a reboot,
without changing service, publication, enrollment or factory state.

## Decision

Determine whether the hub answers now, which boot/process record is observed,
whether publication and boot prerequisites exist, and which factory facts remain
unverified. A running process must never imply resumed jobs.

## Concept

A timestamped receipt with independent component outcomes, evidence sources and
stable reason codes; present configuration is distinct from actual reboot proof.

## Hierarchy

Existing hub status remains the command verdict. The JSON receipt groups current
hub/publication/factory observations, then prerequisites and observation timing.
Missing evidence stays unknown, with the next prerequisite named.

## Machine output

Add `runtime_receipt` to the existing single `cas hub status --json` document,
including missing-record failures. It carries schema version, collection time,
scoped OS boot ID, recorded hub-instance ID, typed observations, factory process
counts, and Linux linger/macOS GUI-login prerequisites. No secrets, raw manager
errors, session names, paths or command lines. No cloud-monitoring claim.

## Critique

Source-only Phase A: fit/hierarchy/craft cannot be scored as runtime PASS until
the supervisor builds and captures the delivered command. Terminal QA and actual
Linux/macOS lifecycle proof are supervisor-owned under explicit no-cargo.

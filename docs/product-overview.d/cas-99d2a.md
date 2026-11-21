## Core Concepts

### Verification against rulings and real output contracts

Delivery review discovers applicable stakeholder and operator rulings even when
the task does not cite them. RULING-FIT rejects implementations and tests that
contradict those rulings. FAKE-SEAM checks mocked component outputs against the
real producer's contract, rejecting impossible fixtures. Both checks cite their
sources and distinguish missing evidence from a known violation.

## Core Concepts

- **Review recovery.** Was: an expired claimed review could keep a merge blocked
  after its verifier died, requiring a manually recorded error. Now: the owning
  supervisor's merge retry can confirm local verifier death and release that exact
  claim for replacement. The candidate and review history stay intact; live or
  unverifiable children remain protected.

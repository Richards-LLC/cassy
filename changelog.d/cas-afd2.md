### Fixed — Task verification

- **Expired task reviews can be requested again** — Was: a verifier timeout
  blocked every later close and left a corrected no-code report behind the
  proof scope lock. Now: close requests a fresh pending review for an expired,
  unparked task-only cycle; a corrected portable `external_ref` can be supplied
  inline and is saved with that dispatch. Active reviews, receipt-bound
  deliveries and epic recovery keep their existing protections.

### Fixed — CI

- **Journey-catalog row explains a missing registry policy** — Was: in a
  checkout with no Cassy store, a registry file without an `enforce:` header
  failed the journey-catalog row with no message once the store lookup
  failed, and an unrecognised stored policy failed the same silent way.
  Registries that declare the header already skip the store. Now: both
  failures still fail closed, and they name the revision whose header is
  missing (or the bad value) and the fix: add the header, or run where the
  store answers.

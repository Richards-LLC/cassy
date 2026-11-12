### Fixed — QA

- **A bug family's statement counts as its case** — Was: extending an existing
  bug family's `statement` was silently dropped, so close refused G3 and asked
  for the change already made. Now: `statement` is a supported bug-family
  field, and a change only to an unsupported key is still refused with a
  refusal that names that key and the supported fields.

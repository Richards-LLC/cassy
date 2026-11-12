### Fixed — QA

- **A bug family's statement counts as its case** — Was: extending an existing
  bug family's `statement` was silently dropped, so close refused G3 and asked
  for the change already made. Now: `statement` is a supported bug-family
  field, and any other bug-family field is refused by registry validation
  (`unsupported_bug_family_field`) in close, lint and impact alike, naming the
  field and the supported ones.

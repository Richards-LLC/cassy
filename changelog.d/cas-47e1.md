### Fixed — Factory guard

- **Workers can list file names under home** — Was: the credential guard
  refused `rg --files` and `find` over a directory above an operator credential
  store as a credential read, though neither opens a file. Now: a name-only
  listing is allowed while its output only reaches text filters; piping names
  to `xargs`, a `read` loop or a substitution, `find -exec`, and any read of a
  store itself are still refused.

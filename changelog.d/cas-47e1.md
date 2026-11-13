### Fixed — Factory guard

- **Workers can list file names under home** — Was: the credential guard
  refused `rg --files` and `find` over a directory above an operator credential
  store as a credential read, though neither opens a file. Now: a name-only
  listing is allowed when its argv parses as `rg --files` or a `find` with only
  name tests, used alone or redirected to a file. Pipelines and command/process
  substitutions retain normal credential inspection, including
  `sort --files0-from=-` and `xargs cat`. `--files` given as an option value
  or after `--`, `find -exec`, and any read of a store itself are still refused.
- **Credential guard recursion and payload checks parse their flags** — Was:
  `tar --exclude --no-recursion`, `grep -d recurse` and `zip -R` over a store's
  parent were allowed, and `bash -oc pipefail '<cmd>'` hid its command. Now:
  they count as recursive reads, and every word after a shell's command flag is
  inspected.

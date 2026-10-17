# `cas update` interactive captures (native, before cas-70a8)

These are historical evidence of the renderer before cas-70a8, kept as captured.

- `base-y-slow-80.txt` / `.json` is the approved-route control from independent QA round 2 of
  cas-70a8. It is `cas 3.62.0 (a4cdf9f)` (`base-version.txt`), run by the supervisor with factory
  identity preserved, through the same driver as
  [../../after/cas-update-interactive](../../after/cas-update-interactive/README.md). It shows
  self_update's chatter, its release-status block and prompt, then its download bar running
  into `Extracting` and an `[ERROR] UpdateError:` line.
- `available-80.txt`, `current-80.txt` and `current-c-locale.txt` are earlier producer captures
  of `cas 3.62.0 (a4cdf9f)`. They show self_update's release chatter, its 137-cell
  release-status block and `Do you want to continue? [Y/n]`, the `Binary Update` block and the
  raw hub text. Provenance: they were made with the implementer's own pty driver, which
  scrubbed the factory identity variables. That route is not the approved one, so read these
  as illustrations of the old output, not as authority-preserving proof.
- `accept-80-round-1.txt` is the same producer route on `cas 3.62.0 (1f7eea4)`, the cas-70a8
  candidate that QA round 1 rejected. It shows the F01 defect: the download bar and the failed
  result on one 160-cell line.

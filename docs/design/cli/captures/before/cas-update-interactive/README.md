# `cas update` interactive captures (native, before cas-70a8)

These come from the same driver and fake release as
[../../after/cas-update-interactive](../../after/cas-update-interactive/README.md).

- `available-80.txt`, `current-80.txt` and `current-c-locale.txt` come from
  `cas 3.62.0 (a4cdf9f)`, the renderer before cas-70a8. They show self_update's release
  chatter, its 137-cell release-status block and `Do you want to continue? [Y/n]`, the
  `Binary Update` block and the raw hub text.
- `accept-80-round-1.txt` comes from `cas 3.62.0 (1f7eea4)`, the cas-70a8 candidate that QA
  round 1 rejected. It is kept as the F01 control: self_update's download bar without a
  newline, with the failed phase result on the same 160-cell line.

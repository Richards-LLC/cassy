### Fixed — Verification

- **Automatic verifier reads a Claude verdict wrapped in prose or a fence** —
  Was: the Claude fallback stopped with "invalid verifier response: expected
  value at line 1 column 1" whenever the model's final text did not start with
  the JSON object, and the printed output was discarded. Now: the verifier takes
  the last JSON object printed, keeps the full stdout beside its stderr log, and
  a refused response's stdout and stderr tails appear in the notice and the
  dispatch audit.

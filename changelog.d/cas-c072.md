### Fixed — Factory

- **Focused proof asks only for runs the capped runner can make** — Was: park's
  changed-test gate could demand runs that never pass. It asked a changed spec
  to be read by another spec, and it sent every test that read a changed locale
  file to a whole-file run. It also asked for related or package runs of Jest
  and Playwright config files, Playwright runs of Vue email templates in
  packages with no tests, and Vitest runs of Playwright fixture servers. Now:
  park and worker-check resolve each uncertain input to the same single scope.
  Changed specs keep their own stability proof. Only tests built from a JSON
  table owe a whole-file run when it changes. Config files and packages with
  no tests owe nothing. A fixture server proves through its own config's suite,
  run under that config.
- **Worker-check explains a rejected run** — Was: a run whose report passed but
  whose process exited nonzero said only "stability failed", and a skipped
  selected test looked like a failed assertion. Now: the refusal names the exit
  status, or says the selected test did not run. `it.each(...)` tables written
  `as const` name their rows, and a table whose rows cannot be named runs its
  whole spec five times. A Playwright setup project that a related run depends
  on no longer counts as an unrelated spec.

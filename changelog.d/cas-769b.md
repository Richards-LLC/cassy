### Fixed — Factory checks

- **Changed-test proof judges the delivered tip** — Before: park and
  `worker-check` could refuse work over tests the delivery never changed. That
  happened with a reverted `it.each` edit, target edits merged into the lane,
  another author's commits on the branch, or an earlier commit that did not
  parse. Now: only tests whose lines the task's own commits changed in the
  delivered tree are selected, measured from the scoped proof base.
- **Parameterized tests are proved, not refused** — Before: a changed
  `it.each`/`describe.each` row stopped close with "dynamic/parameterized test
  title". Now: the table is one stability identity, proved when every row it
  runs passes five times.
- **worker-check runs the JavaScript command workers type** — Before: `npx`,
  `pnpm exec`, `vitest run`, package-relative paths and a suite's own
  `--config` were refused with a Cargo-only message. Now: they run, and any
  other flag is refused with the JavaScript command shape.
- **test-shape stops calling machine data prose** — Before: `data-*`
  attribute state and explicit `CHANGELOG.md` fixture reads were refused as
  prose or Rust source. Now: they pass, and real prose and source-text pins are
  still refused. A test path next to wording, such as
  `id("a.test.ts", "...")`, does not exempt the wording.

### Fixed — Worker proof

- **Capped no-default-feature library checks** — Was: workers could not prove
  a package's no-default-features repair through the capped runner. Now: one
  explicit `--no-default-features` flag is admitted for package-scoped `--lib`
  checks, with separate configuration-bound receipts that close can import.
  Default proof survives a failed no-default retry. Broader feature, target,
  build and identity overrides remain refused; compile proof does not replace
  required tests or the landing-tree suite.

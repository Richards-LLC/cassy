### Changed — Release CI

- **The shipped Linux binary is audited once** — Was: the portable-instruction audit ran in the prebuild and again in the release on the same bytes (134 s + 185 s). Now: the prebuild audits the executable and records its SHA-256 in the artifact; the release checks the digest (under a second) and runs only the launch smoke.

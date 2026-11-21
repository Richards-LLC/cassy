### Fixed — Factory verification

- **Fetch peer artifacts before review** — Automatic verifiers fetch a delivery's committed artifacts from Cloud into the reviewing machine's task directory and verify their size and SHA-256 before starting. Review inputs identify each local file by artifact ID. Missing or corrupt Cloud evidence stops startup with its artifact ID and underlying error, including for no-code closes.

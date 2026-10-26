# `cas factory doctor` required-backend failure

- **First two lines:** The error names missing required backends and gives the rerun command without referring to where the report rows appear.
- **Scannable:** Existing doctor rows retain their status and hint columns; the failure footer stays on one 80-column line.
- **Readable:** The error tells the operator to fix the missing backends, then rerun `cas factory doctor`.
- **Machine output:** `--json` still prints the existing `rows` document on stdout; the failure error remains on stderr.
- **Omitted:** Backend-specific repairs stay in each row's remediation rather than repeating in the summary error.

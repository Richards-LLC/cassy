### Fixed — Factory guards

- **Playwright optional values preserve the browser test tier** — Named-spec runs accept `--shuffle 42`, string or negative seeds, and Playwright 1.65’s other optional-value flags. Values consumed by options never count as named specs; unfiltered runs and `--full` stay refused, and unsupported tokens are named in the refusal.

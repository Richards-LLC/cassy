### Fixed — Hub

- **Hub status and the journal now say why Tailscale publishing failed** — Was: with publishing denied, `cas hub status` printed only "Tailscale Serve: WARN - unavailable; hub remains loopback-only" and the hub wrote nothing to stderr, so the cause was only in `--json` or `hub.log`. Now: status adds a `Cause:` row under the warning, and the running hub writes one stderr line (the journal on Linux) each time the failure cause changes, not on every retry.

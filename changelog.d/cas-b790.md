### Fixed — Commander

- **Pairing refuses the Tailscale Serve shim** — `cas hub pair` now refuses an origin or hub URL that points at the hub's Tailscale Serve shim (for example `127.0.0.1:34501`), before minting an invitation. It names the hub origin to pair with locally and the Tailscale URL for other devices. Previously the pairing succeeded but every Commander call returned 401. `cas hub status` now prints the local `cas hub pair` command and labels the shim port.

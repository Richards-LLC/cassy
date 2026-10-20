# Hub transport refusals

| Field | Contract |
| --- | --- |
| First two lines | A refused start or failed publication names the Tailscale state and tells the operator how to choose loopback. |
| Scannable | CLI and socket are separate labeled rows beneath the cause; the daemon selector is named explicitly. |
| Readable | A stopped daemon requires selecting or starting the signed-in daemon before restarting the hub. |
| Machine output | Status retains one JSON document with the existing `tailscale_serve.status`, `message`, and `remedy` fields; start refusals remain nonzero with stderr diagnostics. |
| Omitted | Raw Tailscale output, account details, auth state, and alternate daemon discovery never appear in these diagnostics. |

The existing `--no-tailscale-serve` flag remains available; `--loopback` is its
discoverable alias. An explicit `TAILSCALE_SOCKET` follows both service and
detached launches. A missing CLI retains the existing loopback fallback;
an installed CLI that is signed out or cannot be probed refuses `hub start`.

The service retries startup publication within one shared deadline. If those
attempts fail, launchd supervises the next attempt; `hub stop` removes the
bootstrap job before stopping any remaining process.

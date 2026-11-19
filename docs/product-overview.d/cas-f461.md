## Core Concepts

- **Hub publication cause** - When Tailscale Serve cannot publish, `cas hub status` shows the cause on a row under the loopback-only warning, and the running hub writes one stderr line (the service journal) each time that cause changes. Retries of the same failure add no further lines.

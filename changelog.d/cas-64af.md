### Fixed — Violet compatibility

- **Follow the connected hub’s message contract** — Cassy supplies message request IDs and reconciles by them only when Violet advertises both capabilities. Older hubs receive unchanged posts, and reconnect refreshes the contract without ever resending an uncertain write.

### Fixed — Commander

- **Check journey fixtures against hub serializers** — Commander tests now reject catalog, lease, session-end, command-receipt and machine-event fixtures that the Rust hub would not emit.

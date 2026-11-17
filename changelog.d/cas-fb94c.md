### Fixed — Security

- **Patched TLS, QUIC, compression and archive dependencies** — Update rustls to 0.23.46, rustls-webpki to 0.103.17, quinn-proto to 0.11.19, lz4_flex to 0.11.7/0.12.3 and tar to 0.4.46. The legacy PostHog dependency still requires rustls 0.21.12 and rustls-webpki 0.101.7, which have no patched releases compatible with its requirements; those advisories remain unresolved.

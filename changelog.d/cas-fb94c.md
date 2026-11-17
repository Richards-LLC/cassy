### Fixed — Security

- **Patched TLS, QUIC, compression and archive dependencies** — Update rustls to 0.23.46, rustls-webpki to 0.103.17, quinn-proto to 0.11.19, lz4_flex to 0.11.7/0.12.3 and tar to 0.4.46. Upgrade PostHog to 0.6 to remove its vulnerable legacy rustls and certificate-validation stack.

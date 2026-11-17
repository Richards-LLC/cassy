### Fixed — Portable Linux releases

- **Keep telemetry's TLS provider portable** — Restore PostHog 0.3.7 to remove AWS-LC from the Linux dependency graph and unblock the portable x86_64 release audit. Keep the other dependency security updates. Known residual: PostHog's older reqwest 0.11/rustls 0.21 stack reintroduces its existing TLS and certificate-validation advisories; a follow-up must replace or update the telemetry client with a ring-compatible option.

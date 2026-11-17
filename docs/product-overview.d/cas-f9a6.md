## Core Concepts

**Portable telemetry dependencies.** The Linux release dependency graph uses the ring TLS provider and excludes AWS-LC. PostHog remains on 0.3.7 while a follow-up replaces its legacy TLS dependency stack; its existing TLS and certificate-validation advisories remain a known residual. Telemetry call sites and the other dependency security updates are unchanged.

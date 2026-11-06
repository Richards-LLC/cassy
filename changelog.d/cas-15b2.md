### Fixed — Rules

- **Retire unpromoted drafts** — Maintenance retires draft rules after thirty days by default, retaining history. Configure the window with `rules.draft_ttl_days`; Proven rules stay untouched.
- **Require repeated evidence for candidates** — Verifiers must cite completed verdicts from two different tasks before recording a rule candidate. Single-verdict and forged citations are refused.

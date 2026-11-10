### Fixed — Releases

- **Automatic updates survive stuck release notices** — Bound release Git probes and post-install notice delivery, retain pending notices by version, and install newer publications before retrying older notices. Notice retries remain valid after a binary upgrade and reuse existing delivery receipts.

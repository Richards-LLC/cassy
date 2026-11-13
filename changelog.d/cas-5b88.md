### Fixed — Releases

- **Reuse green train proof after release documentation changes** — CODEMAP, changelog, release notes and reports can follow a green train run without another full train CI. Pending CODEMAP edits join the release commit. The release verdict uses existing member approvals and the green frozen-train receipt, without a separate batch release review. Missing required approvals still refuse the cut.

- **Prepared retries and completed reports** — Compiled product overview and journey docs retain green train proof on cut retries. Already closed no-code members keep their original external links without being rebound to artifact metadata. Open report deliveries still require valid artifact proof and remain open until approved.

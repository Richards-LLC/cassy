### Fixed — Release cleanup

- **Preserve the open next train** — Post-publish cleanup now deletes only the epic named in that version's committed release preparation record. An open train synced to main survives; missing or ambiguous cut records permit no deletion, and concurrent branch advances remain protected by Git's lease.

### Fixed — Worker lifecycle

- **Merged receipts owe no worker close** — Approved delivery PR merge notices are now quiet receipts instead of instructions to re-close a parked task. Merge/verdict finalization retains ownership; missing approval and delayed completion still route to the delivery's supervisor.

### Fixed — Commander test reliability

- **Remove flaky project navigation journey case** — Deleted the HUB-J14 multi-machine navigation case containing the intermittent Idle-dot assertion after `hub.up("atlas")`. The journey failed at `74f6684dc7` and passed on rerun at `f12f32b35e` in cas-81e3; the remaining cases and product code are unchanged.

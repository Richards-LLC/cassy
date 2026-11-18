### Fixed — Releases

- **Delivery preview scaling** — Reuse indexed hunks and shared ownership walks across release members while preserving task-owned net-range proof.
- **Conventional delivery ownership** — Recognize task IDs in conventional commit prefixes alongside historical changelog evidence, so abandoned attempts do not hide a shipped fix.
- **Installer timeout evidence** — Report the release adapter phase, deadline and receipt when a publication or notice times out; retained operations still retry on the next tick.

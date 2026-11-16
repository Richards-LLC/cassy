### Fixed — Releases

- **Old held deliveries no longer block every release cut** — The cut's zero-would-close check now counts only the tasks this release includes. Older deliveries still held from earlier releases are listed in the cut output and no longer refuse it, so a train whose own tasks all close or are already closed can cut. A held task inside the release still refuses.

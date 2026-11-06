### Added — Commander

- **Preview a Commander delivery against live data** — `cas hub preview --dist <build> --task <id>` serves a candidate Commander build on its own loopback origin. The operator pairs it once; it reads live sessions, the hub refuses every send, launch, stop and setting change from it, and its pairing ends with the preview window (default 120 minutes). While it runs, the task's awaiting-merge row in Commander links to it as Preview this delivery.

### Fixed — Park

- **Unpushed branch refused at park** — Was: a delivery whose branch was never pushed parked, passed two reviews, and failed to merge only afterward with "PUSH FIRST". Now: park refuses first and names `git push -u origin <branch>`.

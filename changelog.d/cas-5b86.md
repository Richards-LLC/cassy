### Fixed — CLI

- **Readable `cas-update` warnings** — Was: warning markers faded on light terminals, `NO_COLOR` still emitted styling, and the install target could run past 80 columns. Now: markers have stronger contrast, `NO_COLOR` disables styling, and the target path appears on its own indented line.

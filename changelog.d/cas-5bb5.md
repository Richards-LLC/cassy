### Fixed — Hub

- **Keep an explicit stop** — `cas update` and install-on-publish used to restart a hub stopped with `cas hub stop`. They now leave it stopped until an explicit start, restart or service install; crashed hubs still recover.

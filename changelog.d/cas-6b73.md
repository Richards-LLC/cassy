### Fixed — Releases

- **Release delivery works on a fresh host** — `cas release installed` and `cas release published` no longer stop with "no such table: known_repos" when this host has never registered a project, for example a new machine or an isolated HOME. They create the empty host registry and record the release for the supplied project.

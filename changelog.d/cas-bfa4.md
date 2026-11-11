### Fixed — Release

- **A blocked Git probe can no longer hang `cas release installed`** — Each tracked project's root lookup ran `git rev-parse --show-toplevel` with no deadline. On a Mac, a Git child that blocked under the install-on-publish LaunchAgent (for example in a project under ~/Documents) held `cas release installed` for 4h40m after the v4.13.0 install, so v4.14.0 never installed. The probe now stops after 10 seconds and the lookup falls back to the directory walk it already uses outside Git.

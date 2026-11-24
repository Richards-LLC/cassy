## Primary Journeys

- **Stable macOS identity on install and update** — Previously, a new Cassy build could ask again for granted privacy permissions. Now per-user installs reuse an automatically created local signing identity in a separate keychain, without a login password or Apple Developer account. If signing fails, Cassy keeps the installed binary and warns that macOS may ask for access again.

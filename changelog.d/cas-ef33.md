### Fixed — Commander

- **Verified operator messages after updates** — A daemon reconnect can refresh its relay credential when an update rotates it during connection. Commands retain operator authentication and messages use prompt delivery without re-pairing. Stale relay credentials are refused before any command or message is forwarded.

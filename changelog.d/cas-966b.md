### Fixed — Commander

- **Codex quota stays fresh without Codex traffic** — Was: the hub learned an account's quota only from requests it proxied, so with no Codex work for a few hours Accounts showed Codex logins as "≤ N%" from hours ago. Now: Cassy also reads the hub's own inspection runs, which its timer takes every 15 minutes through the provider's free usage endpoint, and uses whichever read of each login is newer. Both machines still show one number per login. Cassy only reads from the hub and never starts a run.

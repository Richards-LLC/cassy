### Fixed — Violet

- **A managed Slack reply no longer fails on a dropped Violet connection** — Was:
  the first reply after an idle spell could go out on a connection Violet had
  already closed, and Cassy reported it as "did not post" so the agent had to
  send it again. Now: when Violet confirms it never received the post, Cassy
  reconnects and sends it once more with the same request ID, so it posts once.

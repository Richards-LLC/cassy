### Fixed — Factory supervisor

- **Supervisor startup no longer waits minutes on parked deliveries** — Was: a
  supervisor startup checked every parked delivery's merge and content gates,
  2 to 21 seconds each, so a project with 202 parked deliveries held the
  session for about 21 minutes. Now: the check stops listing after 10 seconds, startup never waits more than 12 (a timeout retries at the next startup), and it lists
  the deliveries it did not check with their commit receipts, and a compacted
  session does not repeat it.
- **Claude supervisors clear at 80% of their real window** — Was: Claude
  transcripts report no context window, so the clear waited for the
  1,000,000-token cap and Claude compacted first (at about 967,000 on a 1M
  window). Now: Claude's status line reports the window and the handoff and
  clear start at 80% of it.

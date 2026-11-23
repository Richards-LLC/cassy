### Fixed — Messages to a busy recipient

- **A message parked behind a busy recipient surfaces at its next tool call** — A worker's blocker sent to a supervisor in the middle of a tool call could sit gated for minutes with no wake attempt. Each daemon re-offer briefly claims the message. A tool call that finished inside that window found nothing, and Cassy then skipped the inbox at every later tool call until other mail arrived. Releasing the claim now re-signals the recipient's inbox, so the next tool call shows the message.

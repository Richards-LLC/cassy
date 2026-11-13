### Fixed — Commander

- **Slash commands sent during a supervisor turn wait instead of failing** — A command such as `/mcp`, `/model` or `/usage` sent from Commander while the supervisor's turn is still open is now queued and typed once when that turn ends. Its card shows Queued until then. Before, Commander showed "Not sent" and the operator had to watch for the turn to end and resend. Drafts in the native prompt and open native dialogs still refuse the command.

### Fixed — Commander

- **Finish waits for your supervisor** — Background shell output no longer makes an idle supervisor refuse Finish. During a model turn, Finish waits in the command queue and runs when the turn ends and the native prompt is empty. Commander keeps watching queued commands and shows the supervisor's reply.

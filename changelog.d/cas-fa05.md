### Fixed — Factory

- **Recover unassigned worker branches after reboot** — Branch-adopting task transfers use the recorded source branch when cleanup cleared its assignee, including a source available only through origin's tracking ref.

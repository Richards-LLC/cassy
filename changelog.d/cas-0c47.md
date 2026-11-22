### Fixed — Director CI notices

- **No repeated "CI unknown" decision when no workflow runs on factory pushes** — In a repository whose GitHub Actions workflows trigger only on pull requests (or whose push filters exclude the factory branch), a parked delivery can never get a CI run. The director now reads the workflow triggers at the delivery tip and no longer asks the supervisor for a "CI unknown" decision on each delivery. A workflow that should run on the push still raises the bounded alert, which clears once CI posts (GH #2273).

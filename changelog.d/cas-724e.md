### Fixed — Commander

- **Reload Cassy sees a same-version rebuild** — Was: reloading a supervisor
  onto a hotfix or dev build that reports the same version answered "Already
  current" and sent nothing. Now: Cassy records each running build's hash
  beside its version, so a different build at the same version reloads, and
  the reload only counts once the supervisor runs the installed build.

## Primary Journeys

Reload Cassy identifies a build by its version and its build hash: each MCP backend records the hash `cas --version` prints in parentheses beside its version. A hotfix or dev build installed at the same version is therefore a reload target rather than "Already current", and a reload succeeds only once the replacement process runs the installed version and build.

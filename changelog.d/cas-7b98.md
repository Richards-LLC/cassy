### Fixed — Factory guards

- **Shell guards read commands the way the shell runs them** — Was: the
  credential, workspace-write and commit guards each parsed shell commands
  their own way. They refused ordinary worker commands: `bash -lc`, a
  background `bash -c '…' bash "$A"` typecheck, `cd -`, `cd "$OLDPWD"`, a
  subshell `cd` before a push, a local counter `n=$((n+1))`,
  `$(date +%s)` arithmetic, `a=(…)` arrays and removing a worktree
  `node_modules` symlink. They also missed some real escapes: `pushd`,
  `env -C`, a `cd` that leaked out of a subshell, credential paths passed as
  `$1`, and parent variables a child shell never receives. Now: one shell
  reading supplies every executed statement, with its working directory and
  variable values, to all three guards.

### Changed — Factory guards

- **Most pre-tool guards warn instead of block** — Was: every pre-tool guard refused the call it matched. Now: only the guards for unsaved work, secrets, policy and role tampering, production database writes, host build caps and Slack through Violet still refuse; every other guard records one counted refusal and lets the call run with a one-line warning.
- **Unparseable shell commands run** — Was: a command the shell recognizer could not parse was refused, and `.env.example` or `.env.test` edits were refused as secrets. Now: such a command runs unless it names a real credential-store path or literally writes or deletes outside the worktree, and template and test env files are editable.

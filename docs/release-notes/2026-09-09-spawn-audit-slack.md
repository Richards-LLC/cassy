# Slack draft — factory spawn audit names the launched CLI (main merge, PR #780)

Channel: #cas-internal. Deploy target: Live on production (main). Reaches installed hosts with the next runtime release.

## User thread

Top-level:
Live on production — User — Was: the line Cassy printed when it started a Codex helper said it was using a Claude account folder. Now: that line names the tool it actually launched and the account folder that tool really uses.

Reply:
• Honest start-up line — Was: every helper start-up printed a Claude account folder, even when the helper was a Codex one, so a quick read suggested the wrong tool had started. Now: the line says which tool started, which model and effort it runs, and which account folder it uses, so what you read matches what is running.

Reported today as "Codex spawns a Claude agent". Under test on the installed 3.21.0 and a fresh build, Codex helpers did start as Codex; only the start-up line was wrong.

## Dev thread

Top-level:
Live on production — Dev — Was: the PTY spawn audit inferred the worker CLI from inherited metadata and always printed "effective Claude account directory". Now: it audits the launched executable and its provider's account env, with a regression pinning cli=codex to the Codex launcher, receipt, and registration (PR #780).

Reply:
• Spawn audit — Was: `cas_pty::pty` derived the account line from `CLAUDE_CONFIG_DIR` regardless of the launched command and did not name the CLI. Now: `worker_spawn_audit` reads the real command (unwrapping the `nice -n` wrapper), maps it to `CLAUDE_CONFIG_DIR` or `CODEX_HOME`, and logs `cli=… model=… effort=… account=… (source)`.
• Routing regression — Was: no test tied request resolution, queue serialization, launcher construction, and registration metadata together. Now: `codex_spawn_routes_match_launcher_receipt_and_registration` covers explicit, per-worker, config-default, standard-lane, and heavy-lane resolution and asserts the launched CLI equals the registered `worker_cli`.

Measured: the reported Claude-for-Codex swap did not reproduce on the installed 3.21.0 or a dev build. Proof: clean-env scoped nextest 17/17; cargo check exit 0. PR #780.

## POSTED
Posted 2026-09-09 14:37Z via the MechaCassy hub to #cas-internal (C0B44GUKDK2):
- User top-level ts 1788964668.184529 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788964668184529 (reply ts 1788964675.107059)
- Dev top-level ts 1788964676.636519 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788964676636519 (reply ts 1788964686.728459)

# Worker checkpoint

- Task: cas-13c6, fix captured `$?` numeric checks and guard quoted regex `=>` data.
- Branch: `factory/true-robin-25-cas-13c6`; latest code commit before checkpoint: `335ea7a5de4be2ddc2d6c428775de5e57fce078a`.
- Implementation: `shell_effects.rs` admits `$?` and names assigned from `$?` as numeric operands to `test -eq`; other operands remain conservative. Added PreToolUse replay, GH #1583 regex regression, and a correctly formatted `changelog.d/cas-13c6.md`.
- Proof at `335ea7a5de4be2ddc2d6c428775de5e57fce078a`: focused `cargo nextest run -p cas --lib -E 'test(cas_e460_literal_shell_bodies_keep_status_pid_and_read_only_monitor_commands) | test(quoted_read_only_regex_keeps_shell_boundaries_gh_1583) | test(cas_3147_agent_shell_cannot_run_operator_policy_commands) | test(factory_bash_redirect_to_bare_tmp_is_denied)'` passed 4 tests in run `b44b6b32-ad2e-48b7-a1ed-11a073b1f63e`.
- Valid ledger: `/home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-13c6/LEDGER.md`; proof capture: `shell-effect-proof.log` in the same directory.
- Close first refused because the changelog fragment lacked the required `### Fixed — <area>` heading and bullet title. Fixed in `335ea7a5...`; branch pushed. The task allows one close retry after correcting gates.
- Remaining: checkpoint commit changes tip, so rerun the four named tests at the new tip; update the ledger `build` and final test receipt, run `cas qa ledger check`, append current-SHA upkeep escape notes, then retry `task action=close` once.
- Separate GH #1583 Bash `.env` read-protection gap is recorded as a separate supervisor-owned follow-up; this task changes only shell-effect analysis.

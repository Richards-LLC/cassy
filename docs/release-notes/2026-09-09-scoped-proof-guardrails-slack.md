# Slack draft — scoped proof covers builtin guardrail tests (main merge, PR #789)

Channel: #cas-internal. Deploy target: Live on production (main). Reaches installed hosts with the next runtime release.

## User thread

Top-level:
Live on production — User — Was: a change to a built-in Cassy skill could pass its own checks and still fail a release check minutes later. Now: the proof a change must show before hand-off includes the guardrail tests that actually read that skill.

Reply:
• Guardrails travel with the skill — Was: editing a built-in skill or one of its reference pages only required the tests that mention the file by its source path, so size limits and required-phrase checks that read the installed copy were skipped, and twice today a release check or merge check caught what the hand-off proof had missed. Now: any built-in skill, reference, or agent edit must prove the cross-flavor, agent-contract, and skill-guardrail tests, plus every test that reads that file under either its source or installed name.
• Lesson kept — Was: the release procedure had no record of this failure shape. Now: the release failure log names it, with a self-test so the entry cannot rot.
• Budget hint where it is needed — Was: the compact supervisor playbook said nothing about its own 2 KB limit. Now: it tells an editor to split new guidance into a separate reference page.

## Dev thread

Top-level:
Live on production — Dev — Was: `check-scoped-test-surface.sh` derived required targets only from changed `cas-cli/tests/*` and `cas-cli/src/*.rs`, so builtin skill/reference/agent paths were invisible to `--proof`. Now: builtin paths require drift, agent-contract, and skill-guardrail targets and discover more by grepping tests for the source path and the installed catalog spelling, failing closed on rg errors (PR #789).

Reply:
• Path-to-test mapping — Was: only changed `cas-cli/tests/**.rs` and `cas-cli/src/**.rs` produced required targets. Now: `is_builtin_skill_or_agent_path` adds `builtin_flavor_drift_test`, `agent_definition_contract_test`, and `factory_codex_skill_guardrails`, then `discover_builtin_test_targets` greps `cas-cli/tests` for the source path and for `builtin_catalog_path_for` (skills/<name>.md → skills/<name>/SKILL.md; references and agents pass through; codex/grok flavor dirs stripped).
• Fail closed — Was: `rg -l -F -- "$path" cas-cli/tests --glob '*.rs' || true` passed `--glob` as a path after `--` and swallowed the exit-2 error. Now: `--glob` precedes `--`, and any rg exit other than 1 aborts the guard with the literal named.
• Learn + self-tests — Was: no failure-log entry for "proof passed, guardrail failed in the gate". Now: `release-gate.sh --learn` entry in all three cas-cut-release mirrors with a self-test fixture; `test-run-scoped-tests.sh` gains cases for a reference edit and a supervisor-body-only edit.

Proof: scoped-test self-test 28/28; release-gate self-test 75/75; Rust guardrail proof 34/34; terminal-qa PASS 11 runs. PR #789.

## POSTED
Posted 2026-09-09 17:45Z via the MechaCassy hub to #cas-internal (C0B44GUKDK2):
- User top-level ts 1788975882.385779 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788975882385779 (reply ts 1788975890.006819)
- Dev top-level ts 1788975892.362429 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788975892362429 (reply ts 1788975906.305659)
